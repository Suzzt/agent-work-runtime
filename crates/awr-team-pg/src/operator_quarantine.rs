//! Schema-owner recovery for active claims and unattributed executions.
//! Complements history-migration, which deliberately refuses these cases.
//! Never forges executor_client_id, actors, completion receipts, or trust grades.
//! Never reachable through client HTTP/MCP.
use crate::operator_access::require_owner_project;
use crate::{PgError, PgResult};
use serde_json::{Map, Value, json};
use tokio_postgres::{Client, Transaction};

const PROTOCOL: &str = "awr-operator-claim-execution-recovery-v1";
const SAMPLE_LIMIT: usize = 50;

fn invalid() -> PgError {
    PgError::Protocol("invalid operator claim/execution recovery request".into())
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

/// Fallback for active claims that cannot be attributed from ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimDisposition {
    Release,
    Quarantine,
}

impl ClaimDisposition {
    pub(crate) fn parse(s: &str) -> PgResult<Self> {
        match s {
            "release" => Ok(Self::Release),
            "quarantine" => Ok(Self::Quarantine),
            _ => Err(invalid()),
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Quarantine => "quarantine",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClaimDecision {
    AttributeAndRelease {
        workstream_id: String,
        ownership_version: i64,
        coordinator_epoch: String,
    },
    Release,
    Quarantine,
    Refuse {
        reason: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExecutionDecision {
    QuarantineUnknown,
    Refuse { reason: &'static str },
}

/// Classify an inspected claim. Never invents actor/session identity.
pub(crate) fn classify_claim(
    state: &str,
    workstream_id: Option<&str>,
    work_id: &str,
    ownership: Option<&OwnershipBinding>,
    project_epoch: &str,
    disposition: ClaimDisposition,
) -> ClaimDecision {
    if state != "active" {
        return ClaimDecision::Refuse {
            reason: "claim_not_active",
        };
    }
    if workstream_id.is_none() {
        if let Some(o) = ownership {
            if !work_id.is_empty() && !project_epoch.is_empty() {
                return ClaimDecision::AttributeAndRelease {
                    workstream_id: o.workstream_id.clone(),
                    ownership_version: o.ownership_version,
                    coordinator_epoch: project_epoch.into(),
                };
            }
        }
    }
    match disposition {
        ClaimDisposition::Release => ClaimDecision::Release,
        ClaimDisposition::Quarantine => ClaimDecision::Quarantine,
    }
}

/// Classify an execution. Silent attribution is always refused.
pub(crate) fn classify_execution(
    workstream_id: Option<&str>,
    state: &str,
    _session_id: Option<&str>,
    _claim_id: Option<&str>,
    has_explicit_executor_client_id: bool,
) -> ExecutionDecision {
    if workstream_id.is_some() {
        return ExecutionDecision::Refuse {
            reason: "attributed_execution_use_execution_reconcile",
        };
    }
    if has_explicit_executor_client_id {
        return ExecutionDecision::Refuse {
            reason: "use_execution_attribution_protocol",
        };
    }
    if matches!(state, "succeeded" | "failed" | "cancelled") {
        return ExecutionDecision::Refuse {
            reason: "terminal_unattributed_execution_requires_explicit_attribution_protocol",
        };
    }
    ExecutionDecision::QuarantineUnknown
}

fn claim_decision_json(
    id: &str,
    work_id: &str,
    state: &str,
    workstream_id: Option<&str>,
    d: &ClaimDecision,
) -> Value {
    let mut m = Map::new();
    m.insert("kind".into(), json!("claim"));
    m.insert("id".into(), json!(id));
    m.insert("work_id".into(), json!(work_id));
    m.insert("state".into(), json!(state));
    m.insert("workstream_id".into(), json!(workstream_id));
    match d {
        ClaimDecision::AttributeAndRelease {
            workstream_id,
            ownership_version,
            coordinator_epoch,
        } => {
            m.insert("action".into(), json!("attribute_and_release"));
            m.insert("target_workstream_id".into(), json!(workstream_id));
            m.insert(
                "ownership_version".into(),
                json!(ownership_version.to_string()),
            );
            m.insert("coordinator_epoch".into(), json!(coordinator_epoch));
            m.insert("forges_identity".into(), json!(false));
        }
        ClaimDecision::Release => {
            m.insert("action".into(), json!("release"));
            m.insert("forges_identity".into(), json!(false));
        }
        ClaimDecision::Quarantine => {
            m.insert("action".into(), json!("quarantine"));
            m.insert("target_state".into(), json!("revoked"));
            m.insert("forges_identity".into(), json!(false));
        }
        ClaimDecision::Refuse { reason } => {
            m.insert("action".into(), json!("refuse"));
            m.insert("reason".into(), json!(reason));
        }
    }
    Value::Object(m)
}

fn execution_decision_json(
    id: &str,
    work_id: &str,
    state: &str,
    workstream_id: Option<&str>,
    d: &ExecutionDecision,
) -> Value {
    match d {
        ExecutionDecision::QuarantineUnknown => json!({
            "kind": "execution",
            "id": id,
            "work_id": work_id,
            "state": state,
            "workstream_id": workstream_id,
            "action": "quarantine_unknown",
            "target_state": "unknown",
            "forges_identity": false,
            "executor_client_id_set": false
        }),
        ExecutionDecision::Refuse { reason } => json!({
            "kind": "execution",
            "id": id,
            "work_id": work_id,
            "state": state,
            "workstream_id": workstream_id,
            "action": "refuse",
            "reason": reason,
            "forges_identity": false
        }),
    }
}

pub struct OperatorQuarantine;

impl OperatorQuarantine {
    /// Dry-run inventory + classification. No writes.
    /// `claim_disposition` is `release` (default) or `quarantine` when ownership
    /// cannot attribute an active claim.
    pub async fn preview(
        client: &mut Client,
        tenant: &str,
        project: &str,
        claim_disposition: &str,
    ) -> PgResult<Value> {
        if ![tenant, project].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        let disposition = ClaimDisposition::parse(claim_disposition)?;
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let operator = require_owner_project(&tx, tenant, project, false).await?;
        let plan = build_plan(&tx, tenant, project, &operator, disposition).await?;
        tx.commit().await?;
        Ok(plan)
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
                "SELECT result_json FROM awr_team.operator_quarantines
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

    /// Apply the exact reviewed plan digests.
    pub async fn apply(
        client: &mut Client,
        tenant: &str,
        project: &str,
        request: &str,
        expected_state: &str,
        expected_plan: &str,
        claim_disposition: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, request].iter().all(|s| identity(s))
            || !hex(expected_state)
            || !hex(expected_plan)
        {
            return Err(invalid());
        }
        let disposition = ClaimDisposition::parse(claim_disposition)?;
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, tenant, project, true).await?;
        let intent_hash = hash(&json!({
            "protocol": PROTOCOL,
            "tenant_id": tenant,
            "project_id": project,
            "expected_state": expected_state,
            "expected_plan": expected_plan,
            "claim_disposition": disposition.as_str()
        }))?;
        if let Some(r) = tx
            .query_opt(
                "SELECT request_hash,result_json FROM awr_team.operator_quarantines
            WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant, &project, &request],
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
        let plan = build_plan(&tx, tenant, project, &operator, disposition).await?;
        if plan["state_digest"].as_str() != Some(expected_state)
            || plan["plan_digest"].as_str() != Some(expected_plan)
        {
            return Err(PgError::PreconditionsChanged);
        }
        let actionable = plan["actionable"].as_array().cloned().unwrap_or_default();
        let mut applied = Vec::new();
        for item in &actionable {
            let kind = item["kind"].as_str().unwrap_or("");
            let id = item["id"].as_str().unwrap_or("");
            let action = item["action"].as_str().unwrap_or("");
            match (kind, action) {
                ("claim", "release") => {
                    let n = tx
                        .execute(
                            "UPDATE awr_team.claims
                            SET state='released', lease_version=lease_version+1
                            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND state='active'",
                            &[&tenant, &project, &id],
                        )
                        .await?;
                    if n != 1 {
                        return Err(PgError::PreconditionsChanged);
                    }
                }
                ("claim", "quarantine") => {
                    let n = tx
                        .execute(
                            "UPDATE awr_team.claims
                            SET state='revoked', lease_version=lease_version+1
                            WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND state='active'",
                            &[&tenant, &project, &id],
                        )
                        .await?;
                    if n != 1 {
                        return Err(PgError::PreconditionsChanged);
                    }
                }
                ("claim", "attribute_and_release") => {
                    let workstream = item["target_workstream_id"].as_str().ok_or_else(invalid)?;
                    let ownership: i64 = item["ownership_version"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(invalid)?;
                    let epoch = item["coordinator_epoch"].as_str().ok_or_else(invalid)?;
                    let n = tx
                        .execute(
                            "UPDATE awr_team.claims
                            SET workstream_id=$4, ownership_version=$5, coordinator_epoch=$6,
                                state='released', lease_version=lease_version+1
                            WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                              AND state='active'
                              AND workstream_id IS NULL AND ownership_version IS NULL
                              AND coordinator_epoch IS NULL",
                            &[&tenant, &project, &id, &workstream, &ownership, &epoch],
                        )
                        .await?;
                    if n != 1 {
                        return Err(PgError::PreconditionsChanged);
                    }
                }
                ("execution", "quarantine_unknown") => {
                    // Keep an unknown/nonterminal outcome without stop evidence.
                    // cancelled would let claim.acquire proceed while effects remain unresolved.
                    let n = tx
                        .execute(
                            "UPDATE awr_team.executions
                            SET state='unknown',
                                cancel_requested=TRUE,
                                unknown_reason='operator_quarantine',
                                execution_version=execution_version+1
                            WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                              AND workstream_id IS NULL
                              AND state NOT IN ('succeeded','failed','cancelled')",
                            &[&tenant, &project, &id],
                        )
                        .await?;
                    if n != 1 {
                        return Err(PgError::PreconditionsChanged);
                    }
                    // Transactional recovery barrier; explicit stop/reconcile clears it.
                    tx.execute(
                        "INSERT INTO awr_team.work_runtime(
                            tenant_id,project_id,scope_id,work_id,state,work_version,last_fence,recovery_blocked)
                         SELECT e.tenant_id,e.project_id,COALESCE(e.scope_id,'main'),e.work_id,
                                'running',1,COALESCE(e.fence,0),TRUE
                         FROM awr_team.executions e
                         WHERE e.tenant_id=$1 AND e.project_id=$2 AND e.id=$3
                         ON CONFLICT (tenant_id,project_id,scope_id,work_id) DO UPDATE
                           SET recovery_blocked=TRUE,
                               last_fence=CASE
                                 WHEN awr_team.work_runtime.last_fence < 9223372036854775807
                                 THEN awr_team.work_runtime.last_fence+1
                                 ELSE awr_team.work_runtime.last_fence END,
                               work_version=CASE
                                 WHEN awr_team.work_runtime.work_version < 9223372036854775807
                                 THEN awr_team.work_runtime.work_version+1
                                 ELSE awr_team.work_runtime.work_version END",
                        &[&tenant, &project, &id],
                    )
                    .await?;
                }
                _ => return Err(invalid()),
            }
            applied.push(item.clone());
        }
        let revision: i64 = tx
            .query_opt(
                "UPDATE awr_team.projects SET project_revision=project_revision+1
            WHERE tenant_id=$1 AND id=$2 AND project_revision<9223372036854775807
            RETURNING project_revision",
                &[&tenant, &project],
            )
            .await?
            .ok_or(PgError::PreconditionsChanged)?
            .get(0);
        let receipt = json!({
            "protocol": PROTOCOL,
            "request_id": request,
            "request_hash": intent_hash,
            "operator_role": operator,
            "tenant_id": tenant,
            "project_id": project,
            "claim_disposition": disposition.as_str(),
            "state_digest": expected_state,
            "plan_digest": expected_plan,
            "project_revision": revision.to_string(),
            "applied": applied,
            "refused": plan["refused"],
            "counts": plan["counts"],
            "identity_forged": false,
            "executor_client_id_invented": false,
            "completion_receipts_modified": false,
            "execution_authorized": false,
            "automatic_resume": false,
            "recovery_barrier_required": true,
            "stop_or_reconcile_required": true,
            "state_basis": "at_commit"
        });
        tx.execute(
            "INSERT INTO awr_team.operator_quarantines(tenant_id,project_id,request_id,request_hash,operator_role,result_json)
            VALUES($1,$2,$3,$4,$5,$6)",
            &[&tenant, &project, &request, &intent_hash, &operator, &receipt],
        )
        .await?;
        let event = json!({
            "operator_role": operator,
            "request_id": request,
            "applied_count": applied.len(),
            "refused_count": plan["refused"].as_array().map(|a| a.len()).unwrap_or(0),
            "claim_disposition": disposition.as_str()
        });
        tx.execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
            VALUES($1,$2,$3,$4,0,'operator.quarantine',$5,$6)",
            &[
                &tenant,
                &project,
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
) -> PgResult<std::collections::BTreeMap<String, OwnershipBinding>> {
    let rows = tx
        .query(
            "SELECT work_id,workstream_id,ownership_version FROM awr_team.workstream_ownership
        WHERE tenant_id=$1 AND project_id=$2 ORDER BY work_id FOR SHARE",
            &[&tenant, &project],
        )
        .await?;
    let mut map = std::collections::BTreeMap::new();
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
    tenant: &str,
    project: &str,
    operator: &str,
    disposition: ClaimDisposition,
) -> PgResult<Value> {
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

    let mut actionable = Vec::new();
    let mut refused = Vec::new();

    let claims = tx
        .query(
            "SELECT id,work_id,state,workstream_id FROM awr_team.claims
        WHERE tenant_id=$1 AND project_id=$2 AND state='active'
        ORDER BY id FOR SHARE",
            &[&tenant, &project],
        )
        .await?;
    for r in claims {
        let id: String = r.get(0);
        let work_id: String = r.get(1);
        let state: String = r.get(2);
        let ws: Option<String> = r.get(3);
        let d = classify_claim(
            &state,
            ws.as_deref(),
            &work_id,
            ownership.get(&work_id),
            &epoch,
            disposition,
        );
        let item = claim_decision_json(&id, &work_id, &state, ws.as_deref(), &d);
        match d {
            ClaimDecision::Refuse { .. } => refused.push(item),
            _ => actionable.push(item),
        }
    }

    let executions = tx
        .query(
            "SELECT id,work_id,state,workstream_id,session_id,claim_id
        FROM awr_team.executions
        WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL
        ORDER BY id FOR SHARE",
            &[&tenant, &project],
        )
        .await?;
    for r in executions {
        let id: String = r.get(0);
        let work_id: String = r.get(1);
        let state: String = r.get(2);
        let ws: Option<String> = r.get(3);
        let session_id: Option<String> = r.get(4);
        let claim_id: Option<String> = r.get(5);
        let d = classify_execution(
            ws.as_deref(),
            &state,
            session_id.as_deref(),
            claim_id.as_deref(),
            false,
        );
        let item = execution_decision_json(&id, &work_id, &state, ws.as_deref(), &d);
        match d {
            ExecutionDecision::Refuse { .. } => refused.push(item),
            _ => actionable.push(item),
        }
    }

    let counts = json!({
        "actionable": actionable.len(),
        "refused": refused.len(),
        "claims_attribute_and_release": count_action(&actionable, "claim", "attribute_and_release"),
        "claims_release": count_action(&actionable, "claim", "release"),
        "claims_quarantine": count_action(&actionable, "claim", "quarantine"),
        "executions_quarantine_unknown": count_action(&actionable, "execution", "quarantine_unknown"),
        "claims_refused": count_kind(&refused, "claim"),
        "executions_refused": count_kind(&refused, "execution")
    });

    let state = json!({
        "tenant_id": tenant,
        "project_id": project,
        "project_status": status,
        "coordinator_epoch": epoch,
        "project_revision": p.get::<_, i64>(2).to_string(),
        "source_snapshot_id": p.get::<_, Option<String>>(3),
        "claim_disposition": disposition.as_str(),
        "active_claim_ids": actionable.iter().chain(refused.iter())
            .filter(|i| i["kind"] == "claim")
            .map(|i| i["id"].as_str().unwrap_or("").to_string())
            .collect::<Vec<_>>(),
        "unattributed_execution_ids": actionable.iter().chain(refused.iter())
            .filter(|i| i["kind"] == "execution")
            .map(|i| i["id"].as_str().unwrap_or("").to_string())
            .collect::<Vec<_>>()
    });

    let plan_body = json!({
        "protocol": PROTOCOL,
        "protocol_version": 1,
        "read_only_preview": true,
        "mutation_on_preview": false,
        "operator_role": operator,
        "tenant_id": tenant,
        "project_id": project,
        "claim_disposition": disposition.as_str(),
        "safe_subset": [
            "active_claim_release",
            "active_claim_quarantine",
            "active_claim_attribute_and_release",
            "unattributed_nonterminal_execution_quarantine_unknown"
        ],
        "unsafe_excluded": [
            "silent_execution_attribution",
            "executor_client_id_invention",
            "use_execution_attribution_for_reviewed_client_id",
            "completion_receipt_rewrite",
            "automatic_resume"
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
        "claim_disposition": disposition.as_str(),
        "state_digest": state_digest,
        "plan_digest": plan_digest,
        "state": state,
        "counts": counts,
        "actionable": actionable,
        "refused": refused,
        "safe_subset": plan_body["safe_subset"],
        "unsafe_excluded": plan_body["unsafe_excluded"],
        "next_action": "Review actionable/refused; apply with exact state_digest, plan_digest, and the same claim_disposition. Executions never receive a forged executor_client_id here; use execution-attribution-* for reviewed client ids."
    }))
}

fn count_kind(items: &[Value], kind: &str) -> usize {
    items.iter().filter(|i| i["kind"] == kind).count()
}

fn count_action(items: &[Value], kind: &str, action: &str) -> usize {
    items
        .iter()
        .filter(|i| i["kind"] == kind && i["action"] == action)
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
    fn unattributed_active_claim_with_ownership_attribute_and_releases() {
        let o = own("ws-1", 2);
        assert_eq!(
            classify_claim(
                "active",
                None,
                "work-a",
                Some(&o),
                "epoch-1",
                ClaimDisposition::Release
            ),
            ClaimDecision::AttributeAndRelease {
                workstream_id: "ws-1".into(),
                ownership_version: 2,
                coordinator_epoch: "epoch-1".into(),
            }
        );
    }

    #[test]
    fn unattributed_active_claim_without_ownership_follows_disposition() {
        assert_eq!(
            classify_claim(
                "active",
                None,
                "work-a",
                None,
                "epoch-1",
                ClaimDisposition::Release
            ),
            ClaimDecision::Release
        );
        assert_eq!(
            classify_claim(
                "active",
                None,
                "work-a",
                None,
                "epoch-1",
                ClaimDisposition::Quarantine
            ),
            ClaimDecision::Quarantine
        );
    }

    #[test]
    fn attributed_active_claim_releases_or_quarantines_without_rewriting_binding() {
        let o = own("ws-1", 1);
        assert_eq!(
            classify_claim(
                "active",
                Some("ws-1"),
                "work-a",
                Some(&o),
                "epoch-1",
                ClaimDisposition::Release
            ),
            ClaimDecision::Release
        );
        assert_eq!(
            classify_claim(
                "active",
                Some("ws-1"),
                "work-a",
                Some(&o),
                "epoch-1",
                ClaimDisposition::Quarantine
            ),
            ClaimDecision::Quarantine
        );
    }

    #[test]
    fn inactive_claims_are_refused() {
        assert_eq!(
            classify_claim(
                "released",
                None,
                "work-a",
                None,
                "epoch",
                ClaimDisposition::Release
            ),
            ClaimDecision::Refuse {
                reason: "claim_not_active"
            }
        );
    }

    #[test]
    fn unattributed_nonterminal_execution_quarantines_without_forging_client() {
        assert_eq!(
            classify_execution(None, "running", Some("s1"), Some("c1"), false),
            ExecutionDecision::QuarantineUnknown
        );
        assert_eq!(
            classify_execution(None, "prepared", None, None, false),
            ExecutionDecision::QuarantineUnknown
        );
    }

    #[test]
    fn terminal_unattributed_and_attributed_executions_are_refused() {
        assert_eq!(
            classify_execution(None, "succeeded", Some("s"), Some("c"), false),
            ExecutionDecision::Refuse {
                reason: "terminal_unattributed_execution_requires_explicit_attribution_protocol"
            }
        );
        assert_eq!(
            classify_execution(Some("ws"), "running", Some("s"), Some("c"), false),
            ExecutionDecision::Refuse {
                reason: "attributed_execution_use_execution_reconcile"
            }
        );
    }

    #[test]
    fn explicit_executor_client_deferred_to_attribution_protocol() {
        assert_eq!(
            classify_execution(None, "running", Some("s"), Some("c"), true),
            ExecutionDecision::Refuse {
                reason: "use_execution_attribution_protocol"
            }
        );
    }

    #[test]
    fn decision_json_never_sets_executor_client_id() {
        let d = ClaimDecision::AttributeAndRelease {
            workstream_id: "ws".into(),
            ownership_version: 1,
            coordinator_epoch: "e".into(),
        };
        let v = claim_decision_json("c1", "w1", "active", None, &d);
        assert!(v.get("executor_client_id").is_none());
        assert_eq!(v["forges_identity"], false);

        let e = execution_decision_json(
            "e1",
            "w1",
            "running",
            None,
            &ExecutionDecision::QuarantineUnknown,
        );
        assert_eq!(e["executor_client_id_set"], false);
        assert_eq!(e["forges_identity"], false);
    }

    #[test]
    fn claim_disposition_parse_rejects_unknown() {
        assert!(ClaimDisposition::parse("release").is_ok());
        assert!(ClaimDisposition::parse("quarantine").is_ok());
        assert!(ClaimDisposition::parse("forge").is_err());
    }
}
