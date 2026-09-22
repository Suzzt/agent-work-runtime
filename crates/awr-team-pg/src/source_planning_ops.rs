//! Authenticated planning ops for HTTP/MCP (AWR-TMCP-023).
//!
//! Wraps TMCP-021/022 domain entrypoints with stable `request_id` receipts so
//! HTTP and MCP share identity, action auth, and idempotent replay. Clients
//! never receive SQL tools, arbitrary filesystem writes, or direct `done`.

use super::planning::{DraftCandidateCreate, SuggestionSubmit};
use super::writeback::WritebackActivateRequest;
use super::{PgError, PgResult, SoleSourceBinding, SoleSourceKind, SourceStore};
use crate::tx::bind_workstream_scope;
use crate::workstream_auth::{authenticate, authorize_domain_action};
use awr_team::DraftChange;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

const RECEIPT_PROTOCOL: &str = "awr-team-planning-command-v1";

fn planning_protocol_v1() -> u32 {
    1
}

fn require_protocol(v: u32) -> PgResult<()> {
    if v != 1 {
        return Err(PgError::Protocol("planning protocol_version must be 1".into()));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningSuggestRequest {
    #[serde(default = "planning_protocol_v1")]
    pub protocol_version: u32,
    pub request_id: String,
    pub rationale: String,
    pub affected_work_keys: Vec<String>,
    #[serde(default)]
    pub proposed_notes: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningDraftRequest {
    #[serde(default = "planning_protocol_v1")]
    pub protocol_version: u32,
    pub request_id: String,
    /// `create` or `edit` (split/cancel/archive are DraftChange ops inside `changes`).
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(default)]
    pub changes: Vec<DraftChange>,
    #[serde(default)]
    pub suggestion_ids: Vec<String>,
    #[serde(default)]
    pub allowed_spec_roots: Vec<String>,
    #[serde(default)]
    pub project_goal_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_approve_policy: Option<awr_team::OrdinaryPlanningSelfApprovePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningApproveRequest {
    #[serde(default = "planning_protocol_v1")]
    pub protocol_version: u32,
    pub request_id: String,
    pub candidate_id: String,
    pub candidate_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningPublishRequest {
    #[serde(default = "planning_protocol_v1")]
    pub protocol_version: u32,
    pub request_id: String,
    pub candidate_id: String,
    pub candidate_digest: String,
    /// When true, also activate writeback using the registered sole source.
    #[serde(default)]
    pub activate: bool,
    #[serde(default)]
    pub impact_proven: bool,
    #[serde(default)]
    pub stopped_work_ids: Vec<String>,
}

fn identity(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}

fn require_request_id(id: &str) -> PgResult<()> {
    if !identity(id) {
        return Err(PgError::Protocol(
            "request_id required (1..128, no controls) for idempotent planning ops".into(),
        ));
    }
    Ok(())
}

fn intent_hash(op: &str, body: &Value) -> PgResult<String> {
    awr_team::request_hash(&json!({"protocol": RECEIPT_PROTOCOL, "op": op, "body": body}))
        .map_err(|_| PgError::Protocol("planning request hash failed".into()))
}

impl SourceStore {
    /// Lookup a prior planning mutation receipt (disconnect recovery).
    pub async fn get_planning_command_receipt(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        request_id: &str,
    ) -> PgResult<Option<Value>> {
        require_request_id(request_id)?;
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        let scope = crate::workstream_auth::authority_scope(&auth, None, None);
        let can = [
            awr_team::Action::PlanningPropose,
            awr_team::Action::PlanningEditDraft,
            awr_team::Action::PlanningApprove,
            awr_team::Action::PlanningPublish,
            awr_team::Action::WorkRead,
        ]
        .iter()
        .any(|a| scope.allowed_actions.contains(a));
        if !can {
            return Err(PgError::Forbidden);
        }
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        let row = tx
            .query_opt(
                "SELECT op, request_hash, actor_id, client_id, result_json, created_at::text
                 FROM awr_team.planning_command_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant_id, &project_id, &request_id],
            )
            .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let out = json!({
            "protocol": RECEIPT_PROTOCOL,
            "request_id": request_id,
            "op": row.get::<_, String>(0),
            "request_hash": row.get::<_, String>(1),
            "actor_id": row.get::<_, String>(2),
            "client_id": row.get::<_, String>(3),
            "result": row.get::<_, Value>(4),
            "created_at": row.get::<_, String>(5),
            "already_recorded": true,
            "next_step": "reuse this receipt; do not resubmit with a new request_id"
        });
        tx.commit().await?;
        Ok(Some(out))
    }

    async fn commit_planning_receipt(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        request_id: &str,
        op: &str,
        hash: &str,
        result: Value,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        bind_workstream_scope(&tx, tenant_id, project_id).await?;
        if let Some(row) = tx
            .query_opt(
                "SELECT request_hash, result_json FROM awr_team.planning_command_receipts
                 WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant_id, &project_id, &request_id],
            )
            .await?
        {
            let prior_hash: String = row.get(0);
            if prior_hash != hash {
                return Err(PgError::IdempotencyConflict);
            }
            let prior: Value = row.get(1);
            tx.commit().await?;
            return Ok(prior);
        }
        let wrapped = json!({
            "protocol": RECEIPT_PROTOCOL,
            "request_id": request_id,
            "op": op,
            "request_hash": hash,
            "result": result,
            "already_recorded": false
        });
        tx.execute(
            "INSERT INTO awr_team.planning_command_receipts(
                tenant_id, project_id, request_id, op, request_hash,
                actor_id, client_id, result_json)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
            &[
                &tenant_id,
                &project_id,
                &request_id,
                &op,
                &hash,
                &auth.actor_id,
                &auth.client_id,
                &wrapped,
            ],
        )
        .await?;
        tx.commit().await?;
        Ok(wrapped)
    }

    pub async fn planning_suggest(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningSuggestRequest,
    ) -> PgResult<Value> {
        let intent = serde_json::to_value(req).map_err(|e| PgError::Protocol(e.to_string()))?;
        require_request_id(&req.request_id)?;
        require_protocol(req.protocol_version)?;
        let hash = intent_hash("planning.propose", &intent)?;
        if let Some(existing) = self
            .get_planning_command_receipt(tenant_id, project_id, bearer, &req.request_id)
            .await?
        {
            if existing["request_hash"] != hash {
                return Err(PgError::IdempotencyConflict);
            }
            return Ok(existing);
        }
        let submit = SuggestionSubmit {
            rationale: req.rationale.clone(),
            affected_work_keys: req.affected_work_keys.clone(),
            proposed_notes: req.proposed_notes.clone(),
            author_person_id: req.author_person_id.clone(),
        };
        let result = self
            .submit_planning_suggestion(tenant_id, project_id, bearer, &submit)
            .await?;
        self.commit_planning_receipt(
            tenant_id,
            project_id,
            bearer,
            &req.request_id,
            "planning.propose",
            &hash,
            result,
        )
        .await
    }

    pub async fn planning_draft(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningDraftRequest,
    ) -> PgResult<Value> {
        let intent = serde_json::to_value(req).map_err(|e| PgError::Protocol(e.to_string()))?;
        require_request_id(&req.request_id)?;
        require_protocol(req.protocol_version)?;
        let hash = intent_hash("planning.edit_draft", &intent)?;
        if let Some(existing) = self
            .get_planning_command_receipt(tenant_id, project_id, bearer, &req.request_id)
            .await?
        {
            if existing["request_hash"] != hash {
                return Err(PgError::IdempotencyConflict);
            }
            return Ok(existing);
        }
        let result = match req.mode.as_str() {
            "create" => {
                let create = DraftCandidateCreate {
                    changes: req.changes.clone(),
                    suggestion_ids: req.suggestion_ids.clone(),
                    allowed_spec_roots: req.allowed_spec_roots.clone(),
                    project_goal_keys: req.project_goal_keys.clone(),
                    self_approve_policy: req.self_approve_policy.clone(),
                    author_person_id: req.author_person_id.clone(),
                };
                self.create_planning_candidate(tenant_id, project_id, bearer, &create)
                    .await?
            }
            "edit" => {
                let candidate_id = req.candidate_id.as_deref().ok_or_else(|| {
                    PgError::Protocol("candidate_id required for draft edit".into())
                })?;
                self.edit_planning_candidate(
                    tenant_id,
                    project_id,
                    bearer,
                    candidate_id,
                    req.changes.clone(),
                )
                .await?
            }
            other => {
                return Err(PgError::Protocol(format!(
                    "unsupported draft mode '{other}'; use create or edit (split/cancel/archive via changes[].op)"
                )));
            }
        };
        self.commit_planning_receipt(
            tenant_id,
            project_id,
            bearer,
            &req.request_id,
            "planning.edit_draft",
            &hash,
            result,
        )
        .await
    }

    pub async fn planning_approve(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningApproveRequest,
    ) -> PgResult<Value> {
        let intent = serde_json::to_value(req).map_err(|e| PgError::Protocol(e.to_string()))?;
        require_request_id(&req.request_id)?;
        require_protocol(req.protocol_version)?;
        let hash = intent_hash("planning.approve", &intent)?;
        if let Some(existing) = self
            .get_planning_command_receipt(tenant_id, project_id, bearer, &req.request_id)
            .await?
        {
            if existing["request_hash"] != hash {
                return Err(PgError::IdempotencyConflict);
            }
            return Ok(existing);
        }
        let result = self
            .approve_planning_candidate(
                tenant_id,
                project_id,
                bearer,
                &req.candidate_id,
                &req.candidate_digest,
                req.author_person_id.as_deref(),
            )
            .await?;
        self.commit_planning_receipt(
            tenant_id,
            project_id,
            bearer,
            &req.request_id,
            "planning.approve",
            &hash,
            result,
        )
        .await
    }

    pub async fn planning_publish(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        req: &PlanningPublishRequest,
    ) -> PgResult<Value> {
        let intent = serde_json::to_value(req).map_err(|e| PgError::Protocol(e.to_string()))?;
        require_request_id(&req.request_id)?;
        require_protocol(req.protocol_version)?;
        let op = if req.activate {
            "planning.activate"
        } else {
            "planning.publish"
        };
        let hash = intent_hash(op, &intent)?;
        if let Some(existing) = self
            .get_planning_command_receipt(tenant_id, project_id, bearer, &req.request_id)
            .await?
        {
            if existing["request_hash"] != hash {
                return Err(PgError::IdempotencyConflict);
            }
            return Ok(existing);
        }
        let published = self
            .publish_planning_candidate(
                tenant_id,
                project_id,
                bearer,
                &req.candidate_id,
                &req.candidate_digest,
            )
            .await?;
        let result = if req.activate {
            let receipt_id = published["receipt_id"]
                .as_str()
                .ok_or_else(|| PgError::Protocol("publish receipt missing".into()))?;
            let activated = self
                .activate_planning_writeback_registered(
                    tenant_id,
                    project_id,
                    bearer,
                    &req.request_id,
                    receipt_id,
                    req.impact_proven,
                    &req.stopped_work_ids,
                )
                .await?;
            json!({"publish": published, "activation": activated})
        } else {
            json!({
                "publish": published,
                "activation": null,
                "next_step": "query planning.outcome with this request_id after disconnect; to activate, resubmit same request_id only after stop/reconcile of affected in-flight work with activate=true"
            })
        };
        self.commit_planning_receipt(
            tenant_id,
            project_id,
            bearer,
            &req.request_id,
            op,
            &hash,
            result,
        )
        .await
    }

    /// Activate using the project's registered sole source — never a client path/URL.
    pub async fn activate_planning_writeback_registered(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        request_id: &str,
        publish_receipt_id: &str,
        impact_proven: bool,
        stopped_work_ids: &[String],
    ) -> PgResult<Value> {
        let binding = self
            .load_registered_sole_source(tenant_id, project_id, bearer)
            .await?;
        match binding.kind {
            SoleSourceKind::ServerDirectory => {
                let ledger = binding
                    .ledger_relative_path
                    .clone()
                    .unwrap_or_else(|| "ledger.yaml".into());
                if ledger.contains("..")
                    || ledger.starts_with('/')
                    || ledger.contains('\\')
                    || ledger.contains(':')
                {
                    return Err(PgError::UnsafeSourcePath(ledger));
                }
                let req = WritebackActivateRequest {
                    request_id: request_id.into(),
                    publish_receipt_id: publish_receipt_id.into(),
                    source_root: PathBuf::from(&binding.locator),
                    ledger_relative_path: ledger,
                    impact_proven,
                    stopped_work_ids: stopped_work_ids.to_vec(),
                };
                self.activate_planning_writeback(tenant_id, project_id, bearer, &req)
                    .await
            }
            SoleSourceKind::PrivateManagementRepo => Err(PgError::Unsupported(
                "private management repo writeback adapter is not enabled for MCP/HTTP; register a server_directory sole source or use the operator-local writeback path".into(),
            )),
        }
    }

    async fn load_registered_sole_source(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
    ) -> PgResult<SoleSourceBinding> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        authorize_domain_action(&auth, awr_team::Action::PlanningPublish, None, None)?;
        let row = tx
            .query_opt(
                "SELECT s.source_ref_json
                 FROM awr_team.projects p
                 JOIN awr_team.source_snapshots s
                   ON s.tenant_id=p.tenant_id AND s.project_id=p.id
                  AND s.id=p.active_snapshot_id
                 WHERE p.tenant_id=$1 AND p.id=$2",
                &[&tenant_id, &project_id],
            )
            .await?
            .ok_or_else(|| {
                PgError::Protocol(
                    "no active source snapshot; ingest and activate a sole-source package first"
                        .into(),
                )
            })?;
        let source_ref: Value = row.get(0);
        let sole = source_ref.get("sole_source").cloned().ok_or_else(|| {
            PgError::Protocol("active source lacks sole_source binding".into())
        })?;
        if sole.is_null() {
            return Err(PgError::Protocol(
                "active source lacks sole_source binding".into(),
            ));
        }
        let binding: SoleSourceBinding = serde_json::from_value(sole)
            .map_err(|e| PgError::Protocol(format!("invalid registered sole_source: {e}")))?;
        tx.commit().await?;
        Ok(binding)
    }
}
