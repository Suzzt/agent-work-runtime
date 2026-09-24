//! Explicit schema-owner attribution of unattributed executions with a reviewed
//! `executor_client_id`. Complements history-migration and quarantine, which refuse
//! silent invention. Never reachable through client HTTP/MCP.
//! Never forges actors, completion receipts, trust grades, session/claim ids, or
//! invents executor_client_id; CHECK-safe binding only.
use crate::operator_access::require_owner_project;
use crate::{PgError, PgResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tokio_postgres::{Client, Transaction};

const PROTOCOL: &str = "awr-operator-execution-attribution-v1";
const SAMPLE_LIMIT: usize = 50;
const MAX_ATTRIBUTIONS: usize = 256;

fn invalid() -> PgError {
    PgError::Protocol("invalid operator execution attribution request".into())
}
fn identity(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn hash(v: &Value) -> PgResult<String> {
    awr_team::request_hash(v).map_err(|_| invalid())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnershipBinding {
    pub workstream_id: String,
    pub ownership_version: i64,
}

/// One reviewed attribution entry. Operator must supply executor_client_id explicitly.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAttributionEntry {
    pub execution_id: String,
    pub executor_client_id: String,
}

/// Reviewed plan: only listed executions are considered; client ids are never inferred.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAttributionPlan {
    pub protocol_version: u32,
    pub tenant_id: String,
    pub project_id: String,
    pub attributions: Vec<ExecutionAttributionEntry>,
}

impl ExecutionAttributionPlan {
    pub fn validate(&self) -> PgResult<()> {
        if self.protocol_version != 1
            || !identity(&self.tenant_id)
            || !identity(&self.project_id)
            || self.attributions.is_empty()
            || self.attributions.len() > MAX_ATTRIBUTIONS
        {
            return Err(invalid());
        }
        let mut seen = BTreeSet::new();
        for a in &self.attributions {
            if !identity(&a.execution_id) || !identity(&a.executor_client_id) {
                return Err(invalid());
            }
            if !seen.insert(a.execution_id.clone()) {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn reviewed_map(&self) -> BTreeMap<&str, &str> {
        self.attributions
            .iter()
            .map(|a| (a.execution_id.as_str(), a.executor_client_id.as_str()))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AttributionDecision {
    Attribute {
        workstream_id: String,
        ownership_version: i64,
        coordinator_epoch: String,
        executor_client_id: String,
    },
    Refuse {
        reason: &'static str,
    },
}

/// Classify one unattributed (or candidate) execution.
/// Safe subset: session_id + claim_id present (CHECK), ownership binds work_id,
/// reviewed executor_client_id matches the recorded session client_id.
/// Terminal states are attributable when CHECK-safe; unsafe terminals are refused
/// with an explicit reason (never silently skipped into inventing identity).
pub(crate) fn classify_execution_attribution(
    workstream_id: Option<&str>,
    _state: &str,
    session_id: Option<&str>,
    claim_id: Option<&str>,
    ownership: Option<&OwnershipBinding>,
    project_epoch: &str,
    reviewed_executor_client_id: Option<&str>,
    session_client_id: Option<&str>,
) -> AttributionDecision {
    if workstream_id.is_some() {
        return AttributionDecision::Refuse {
            reason: "already_attributed_use_execution_reconcile",
        };
    }
    let Some(reviewed) = reviewed_executor_client_id.filter(|s| !s.is_empty()) else {
        return AttributionDecision::Refuse {
            reason: "reviewed_executor_client_id_required",
        };
    };
    match session_id {
        None | Some("") => {
            return AttributionDecision::Refuse {
                reason: "check_requires_session_id",
            };
        }
        Some(_) => {}
    }
    match claim_id {
        None | Some("") => {
            return AttributionDecision::Refuse {
                reason: "check_requires_claim_id",
            };
        }
        Some(_) => {}
    }
    let Some(session_client) = session_client_id.filter(|s| !s.is_empty()) else {
        return AttributionDecision::Refuse {
            reason: "session_missing_cannot_verify_executor_client_id",
        };
    };
    if session_client != reviewed {
        return AttributionDecision::Refuse {
            reason: "reviewed_executor_client_id_mismatch_session",
        };
    }
    let Some(o) = ownership else {
        return AttributionDecision::Refuse {
            reason: "work_id_missing_from_current_ownership",
        };
    };
    if project_epoch.is_empty() {
        return AttributionDecision::Refuse {
            reason: "project_coordinator_epoch_missing",
        };
    }
    AttributionDecision::Attribute {
        workstream_id: o.workstream_id.clone(),
        ownership_version: o.ownership_version,
        coordinator_epoch: project_epoch.into(),
        executor_client_id: reviewed.into(),
    }
}

fn decision_json(
    id: &str,
    work_id: &str,
    state: &str,
    session_id: Option<&str>,
    claim_id: Option<&str>,
    session_client_id: Option<&str>,
    d: &AttributionDecision,
) -> Value {
    let mut m = Map::new();
    m.insert("kind".into(), json!("execution"));
    m.insert("id".into(), json!(id));
    m.insert("work_id".into(), json!(work_id));
    m.insert("state".into(), json!(state));
    m.insert("session_id".into(), json!(session_id));
    m.insert("claim_id".into(), json!(claim_id));
    m.insert(
        "observed_session_client_id".into(),
        json!(session_client_id),
    );
    match d {
        AttributionDecision::Attribute {
            workstream_id,
            ownership_version,
            coordinator_epoch,
            executor_client_id,
        } => {
            m.insert("action".into(), json!("attribute"));
            m.insert("target_workstream_id".into(), json!(workstream_id));
            m.insert(
                "ownership_version".into(),
                json!(ownership_version.to_string()),
            );
            m.insert("coordinator_epoch".into(), json!(coordinator_epoch));
            m.insert("executor_client_id".into(), json!(executor_client_id));
            m.insert("forges_identity".into(), json!(false));
            m.insert("executor_client_id_invented".into(), json!(false));
            m.insert("completion_receipts_modified".into(), json!(false));
        }
        AttributionDecision::Refuse { reason } => {
            m.insert("action".into(), json!("refuse"));
            m.insert("reason".into(), json!(reason));
            m.insert("forges_identity".into(), json!(false));
            m.insert("executor_client_id_invented".into(), json!(false));
        }
    }
    Value::Object(m)
}

pub struct OperatorExecutionAttribution;

impl OperatorExecutionAttribution {
    /// Dry-run classification against a reviewed attribution plan. No writes.
    pub async fn preview(client: &mut Client, plan: &ExecutionAttributionPlan) -> PgResult<Value> {
        plan.validate()?;
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let operator = require_owner_project(&tx, &plan.tenant_id, &plan.project_id, false).await?;
        let built = build_plan(&tx, plan, &operator).await?;
        tx.commit().await?;
        Ok(built)
    }

    pub async fn outcome(
        client: &mut Client,
        tenant: &str,
        project: &str,
        request: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, request].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        require_owner_project(&tx, tenant, project, false).await?;
        let row = tx
            .query_opt(
                "SELECT result_json FROM awr_team.execution_attributions
            WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant, &project, &request],
            )
            .await?;
        let result = match row {
            Some(r) => json!({"outcome":"committed","receipt":r.get::<_,Value>(0)}),
            None => json!({"outcome":"unknown"}),
        };
        tx.commit().await?;
        Ok(result)
    }

    /// Apply the exact reviewed plan digests. Sets CHECK-safe attribution only.
    pub async fn apply(
        client: &mut Client,
        plan: &ExecutionAttributionPlan,
        request: &str,
        expected_state: &str,
        expected_plan: &str,
    ) -> PgResult<Value> {
        plan.validate()?;
        if !identity(request) || !hex(expected_state) || !hex(expected_plan) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, &plan.tenant_id, &plan.project_id, true).await?;
        let intent_hash = hash(&json!({
            "protocol": PROTOCOL,
            "tenant_id": plan.tenant_id,
            "project_id": plan.project_id,
            "expected_state": expected_state,
            "expected_plan": expected_plan,
            "plan": serde_json::to_value(plan).map_err(|_| invalid())?
        }))?;
        if let Some(r) = tx
            .query_opt(
                "SELECT request_hash,result_json FROM awr_team.execution_attributions
            WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&plan.tenant_id, &plan.project_id, &request],
            )
            .await?
        {
            if r.get::<_, String>(0) != intent_hash {
                return Err(PgError::IdempotencyConflict);
            }
            let receipt: Value = r.get(1);
            tx.commit().await?;
            return Ok(json!({"replayed":true,"receipt":receipt}));
        }
        let built = build_plan(&tx, plan, &operator).await?;
        if built["state_digest"].as_str() != Some(expected_state)
            || built["plan_digest"].as_str() != Some(expected_plan)
        {
            return Err(PgError::PreconditionsChanged);
        }
        let actionable = built["actionable"].as_array().cloned().unwrap_or_default();
        let mut applied = Vec::new();
        for item in &actionable {
            if item["action"].as_str() != Some("attribute") {
                return Err(invalid());
            }
            let id = item["id"].as_str().ok_or_else(invalid)?;
            let workstream = item["target_workstream_id"].as_str().ok_or_else(invalid)?;
            let ownership: i64 = item["ownership_version"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .ok_or_else(invalid)?;
            let epoch = item["coordinator_epoch"].as_str().ok_or_else(invalid)?;
            let executor = item["executor_client_id"].as_str().ok_or_else(invalid)?;
            let n = tx
                .execute(
                    "UPDATE awr_team.executions
                    SET workstream_id=$4, ownership_version=$5, executor_client_id=$6,
                        coordinator_epoch=$7, execution_version=execution_version+1
                    WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                      AND workstream_id IS NULL AND ownership_version IS NULL
                      AND executor_client_id IS NULL
                      AND session_id IS NOT NULL AND claim_id IS NOT NULL
                      AND session_id <> '' AND claim_id <> ''",
                    &[
                        &plan.tenant_id,
                        &plan.project_id,
                        &id,
                        &workstream,
                        &ownership,
                        &executor,
                        &epoch,
                    ],
                )
                .await?;
            if n != 1 {
                return Err(PgError::PreconditionsChanged);
            }
            applied.push(item.clone());
        }
        let revision: i64 = tx
            .query_opt(
                "UPDATE awr_team.projects SET project_revision=project_revision+1
            WHERE tenant_id=$1 AND id=$2 AND project_revision<9223372036854775807
            RETURNING project_revision",
                &[&plan.tenant_id, &plan.project_id],
            )
            .await?
            .ok_or(PgError::PreconditionsChanged)?
            .get(0);
        let receipt = json!({
            "protocol": PROTOCOL,
            "request_id": request,
            "request_hash": intent_hash,
            "operator_role": operator,
            "tenant_id": plan.tenant_id,
            "project_id": plan.project_id,
            "state_digest": expected_state,
            "plan_digest": expected_plan,
            "project_revision": revision.to_string(),
            "applied": applied,
            "refused": built["refused"],
            "counts": built["counts"],
            "identity_forged": false,
            "executor_client_id_invented": false,
            "completion_receipts_modified": false,
            "execution_authorized": false,
            "automatic_resume": false,
            "state_basis": "at_commit"
        });
        tx.execute(
            "INSERT INTO awr_team.execution_attributions(tenant_id,project_id,request_id,request_hash,operator_role,result_json)
            VALUES($1,$2,$3,$4,$5,$6)",
            &[
                &plan.tenant_id,
                &plan.project_id,
                &request,
                &intent_hash,
                &operator,
                &receipt,
            ],
        )
        .await?;
        let event = json!({
            "operator_role": operator,
            "request_id": request,
            "applied_count": applied.len(),
            "refused_count": built["refused"].as_array().map(|a| a.len()).unwrap_or(0)
        });
        tx.execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
            VALUES($1,$2,$3,$4,0,'operator.execution_attribution',$5,$6)",
            &[
                &plan.tenant_id,
                &plan.project_id,
                &crate::tx::new_id(),
                &revision,
                &format!("operator:{operator}"),
                &event,
            ],
        )
        .await?;
        tx.commit().await?;
        Ok(json!({"replayed":false,"receipt":receipt}))
    }
}

async fn load_ownership(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<BTreeMap<String, OwnershipBinding>> {
    let rows = tx
        .query(
            "SELECT work_id,workstream_id,ownership_version FROM awr_team.workstream_ownership
        WHERE tenant_id=$1 AND project_id=$2 ORDER BY work_id FOR SHARE",
            &[&tenant, &project],
        )
        .await?;
    let mut map = BTreeMap::new();
    for r in rows {
        map.insert(
            r.get::<_, String>(0),
            OwnershipBinding {
                workstream_id: r.get(1),
                ownership_version: r.get(2),
            },
        );
    }
    Ok(map)
}

async fn build_plan(
    tx: &Transaction<'_>,
    plan: &ExecutionAttributionPlan,
    operator: &str,
) -> PgResult<Value> {
    let tenant = plan.tenant_id.as_str();
    let project = plan.project_id.as_str();
    let p = tx
        .query_one(
            "SELECT status,coordinator_epoch,project_revision,active_snapshot_id
        FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &project],
        )
        .await?;
    let status: String = p.get(0);
    if status != "active" {
        return Err(PgError::ProjectNotAvailable);
    }
    let epoch: String = p.get(1);
    let ownership = load_ownership(tx, tenant, project).await?;
    let reviewed = plan.reviewed_map();

    let mut actionable = Vec::new();
    let mut refused = Vec::new();
    let mut missing_from_db = Vec::new();

    for entry in &plan.attributions {
        let row = tx
            .query_opt(
                "SELECT id,work_id,state,workstream_id,session_id,claim_id
            FROM awr_team.executions
            WHERE tenant_id=$1 AND project_id=$2 AND id=$3
            FOR SHARE",
                &[&tenant, &project, &entry.execution_id],
            )
            .await?;
        let Some(r) = row else {
            missing_from_db.push(json!({
                "kind": "execution",
                "id": entry.execution_id,
                "action": "refuse",
                "reason": "execution_not_found",
                "forges_identity": false,
                "executor_client_id_invented": false
            }));
            continue;
        };
        let id: String = r.get(0);
        let work_id: String = r.get(1);
        let state: String = r.get(2);
        let ws: Option<String> = r.get(3);
        let session_id: Option<String> = r.get(4);
        let claim_id: Option<String> = r.get(5);
        let session_client: Option<String> = if let Some(ref sid) = session_id {
            tx.query_opt(
                "SELECT client_id FROM awr_team.sessions
                WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, sid],
            )
            .await?
            .map(|s| s.get(0))
        } else {
            None
        };
        let reviewed_id = reviewed.get(id.as_str()).copied();
        let d = classify_execution_attribution(
            ws.as_deref(),
            &state,
            session_id.as_deref(),
            claim_id.as_deref(),
            ownership.get(&work_id),
            &epoch,
            reviewed_id,
            session_client.as_deref(),
        );
        let item = decision_json(
            &id,
            &work_id,
            &state,
            session_id.as_deref(),
            claim_id.as_deref(),
            session_client.as_deref(),
            &d,
        );
        match d {
            AttributionDecision::Refuse { .. } => refused.push(item),
            AttributionDecision::Attribute { .. } => actionable.push(item),
        }
    }
    refused.extend(missing_from_db);

    let counts = json!({
        "actionable": actionable.len(),
        "refused": refused.len(),
        "executions_attribute": count_action(&actionable, "attribute"),
        "refused_check_requires_session_id": count_reason(&refused, "check_requires_session_id"),
        "refused_check_requires_claim_id": count_reason(&refused, "check_requires_claim_id"),
        "refused_session_missing": count_reason(&refused, "session_missing_cannot_verify_executor_client_id"),
        "refused_client_mismatch": count_reason(&refused, "reviewed_executor_client_id_mismatch_session"),
        "refused_missing_ownership": count_reason(&refused, "work_id_missing_from_current_ownership"),
        "refused_already_attributed": count_reason(&refused, "already_attributed_use_execution_reconcile"),
        "refused_not_found": count_reason(&refused, "execution_not_found")
    });

    let plan_value = serde_json::to_value(plan).map_err(|_| invalid())?;
    let state = json!({
        "tenant_id": tenant,
        "project_id": project,
        "project_status": status,
        "coordinator_epoch": epoch,
        "project_revision": p.get::<_, i64>(2).to_string(),
        "source_snapshot_id": p.get::<_, Option<String>>(3),
        "attribution_execution_ids": plan.attributions.iter().map(|a| a.execution_id.clone()).collect::<Vec<_>>(),
        "attribution_executor_client_ids": plan.attributions.iter().map(|a| a.executor_client_id.clone()).collect::<Vec<_>>()
    });

    let plan_body = json!({
        "protocol": PROTOCOL,
        "protocol_version": 1,
        "read_only_preview": true,
        "mutation_on_preview": false,
        "operator_role": operator,
        "tenant_id": tenant,
        "project_id": project,
        "plan": plan_value,
        "safe_subset": [
            "unattributed_execution_with_session_and_claim",
            "current_ownership_binding",
            "reviewed_executor_client_id_matching_session"
        ],
        "unsafe_excluded": [
            "silent_executor_client_id_invention",
            "attribution_without_session_or_claim",
            "completion_receipt_rewrite",
            "automatic_resume",
            "execution_authorization"
        ],
        "forges_identity": false,
        "local_file_access": "not_server_acl_or_confidentiality_sandbox",
        "frontend_filtering": false,
        "counts": counts,
        "actionable": actionable,
        "refused": refused,
        "actionable_sample": actionable.iter().take(SAMPLE_LIMIT).cloned().collect::<Vec<_>>(),
        "refused_sample": refused.iter().take(SAMPLE_LIMIT).cloned().collect::<Vec<_>>()
    });
    let state_digest = hash(&state)?;
    let plan_digest = hash(&plan_body)?;
    Ok(json!({
        "protocol": PROTOCOL,
        "applied": false,
        "operator_role": operator,
        "state_digest": state_digest,
        "plan_digest": plan_digest,
        "state": state,
        "counts": counts,
        "actionable": actionable,
        "refused": refused,
        "safe_subset": plan_body["safe_subset"],
        "unsafe_excluded": plan_body["unsafe_excluded"],
        "next_action": "Review actionable/refused; apply with exact state_digest and plan_digest. executor_client_id must be reviewed and match the session; it is never invented."
    }))
}

fn count_action(items: &[Value], action: &str) -> usize {
    items.iter().filter(|i| i["action"] == action).count()
}

fn count_reason(items: &[Value], reason: &str) -> usize {
    items
        .iter()
        .filter(|i| i["reason"].as_str() == Some(reason))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own(ws: &str, ver: i64) -> OwnershipBinding {
        OwnershipBinding {
            workstream_id: ws.into(),
            ownership_version: ver,
        }
    }

    #[test]
    fn check_safe_with_matching_reviewed_client_attributes() {
        let o = own("ws-1", 3);
        assert_eq!(
            classify_execution_attribution(
                None,
                "succeeded",
                Some("sess-1"),
                Some("claim-1"),
                Some(&o),
                "epoch-1",
                Some("coding-client"),
                Some("coding-client"),
            ),
            AttributionDecision::Attribute {
                workstream_id: "ws-1".into(),
                ownership_version: 3,
                coordinator_epoch: "epoch-1".into(),
                executor_client_id: "coding-client".into(),
            }
        );
        // Nonterminal CHECK-safe is also attributable (alternative to quarantine-cancel).
        assert_eq!(
            classify_execution_attribution(
                None,
                "running",
                Some("sess-1"),
                Some("claim-1"),
                Some(&o),
                "epoch-1",
                Some("coding-client"),
                Some("coding-client"),
            )
            .is_attribute(),
            true
        );
    }

    #[test]
    fn refuses_missing_reviewed_client_and_never_infers_from_session() {
        let o = own("ws-1", 1);
        assert_eq!(
            classify_execution_attribution(
                None,
                "succeeded",
                Some("sess"),
                Some("claim"),
                Some(&o),
                "epoch",
                None,
                Some("coding-client"),
            ),
            AttributionDecision::Refuse {
                reason: "reviewed_executor_client_id_required"
            }
        );
    }

    #[test]
    fn refuses_check_unsafe_missing_session_or_claim() {
        let o = own("ws-1", 1);
        assert_eq!(
            classify_execution_attribution(
                None,
                "failed",
                None,
                Some("claim"),
                Some(&o),
                "epoch",
                Some("c"),
                Some("c"),
            ),
            AttributionDecision::Refuse {
                reason: "check_requires_session_id"
            }
        );
        assert_eq!(
            classify_execution_attribution(
                None,
                "cancelled",
                Some("sess"),
                None,
                Some(&o),
                "epoch",
                Some("c"),
                Some("c"),
            ),
            AttributionDecision::Refuse {
                reason: "check_requires_claim_id"
            }
        );
    }

    #[test]
    fn refuses_session_missing_and_client_mismatch() {
        let o = own("ws-1", 1);
        assert_eq!(
            classify_execution_attribution(
                None,
                "succeeded",
                Some("sess"),
                Some("claim"),
                Some(&o),
                "epoch",
                Some("reviewed"),
                None,
            ),
            AttributionDecision::Refuse {
                reason: "session_missing_cannot_verify_executor_client_id"
            }
        );
        assert_eq!(
            classify_execution_attribution(
                None,
                "succeeded",
                Some("sess"),
                Some("claim"),
                Some(&o),
                "epoch",
                Some("reviewed"),
                Some("other-client"),
            ),
            AttributionDecision::Refuse {
                reason: "reviewed_executor_client_id_mismatch_session"
            }
        );
    }

    #[test]
    fn refuses_already_attributed_and_missing_ownership() {
        let o = own("ws-1", 1);
        assert_eq!(
            classify_execution_attribution(
                Some("ws-1"),
                "succeeded",
                Some("s"),
                Some("c"),
                Some(&o),
                "epoch",
                Some("client"),
                Some("client"),
            ),
            AttributionDecision::Refuse {
                reason: "already_attributed_use_execution_reconcile"
            }
        );
        assert_eq!(
            classify_execution_attribution(
                None,
                "succeeded",
                Some("s"),
                Some("c"),
                None,
                "epoch",
                Some("client"),
                Some("client"),
            ),
            AttributionDecision::Refuse {
                reason: "work_id_missing_from_current_ownership"
            }
        );
    }

    #[test]
    fn decision_json_records_reviewed_client_without_invention_flag() {
        let d = AttributionDecision::Attribute {
            workstream_id: "ws".into(),
            ownership_version: 2,
            coordinator_epoch: "e".into(),
            executor_client_id: "coding-client".into(),
        };
        let v = decision_json(
            "e1",
            "w1",
            "succeeded",
            Some("s1"),
            Some("c1"),
            Some("coding-client"),
            &d,
        );
        assert_eq!(v["action"], "attribute");
        assert_eq!(v["executor_client_id"], "coding-client");
        assert_eq!(v["executor_client_id_invented"], false);
        assert_eq!(v["forges_identity"], false);
        assert_eq!(v["completion_receipts_modified"], false);
    }

    #[test]
    fn plan_validate_rejects_empty_duplicates_and_bad_ids() {
        let mut plan = ExecutionAttributionPlan {
            protocol_version: 1,
            tenant_id: "t".into(),
            project_id: "p".into(),
            attributions: vec![],
        };
        assert!(plan.validate().is_err());
        plan.attributions.push(ExecutionAttributionEntry {
            execution_id: "e1".into(),
            executor_client_id: "c1".into(),
        });
        assert!(plan.validate().is_ok());
        plan.attributions.push(ExecutionAttributionEntry {
            execution_id: "e1".into(),
            executor_client_id: "c2".into(),
        });
        assert!(plan.validate().is_err());
    }

    impl AttributionDecision {
        fn is_attribute(&self) -> bool {
            matches!(self, Self::Attribute { .. })
        }
    }
}
