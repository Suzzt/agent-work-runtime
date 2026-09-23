//! Explicit schema-owner migration of unattributed workstream history.
//! Opt-in plan/apply only; never runs from recovery inspection or client HTTP/MCP.
//! Does not forge completion receipts, trust grades, actors, or executor identity.
use crate::operator_access::require_owner_project;
use crate::{PgError, PgResult};
use serde_json::{Map, Value, json};
use tokio_postgres::{Client, Transaction};

const PROTOCOL: &str = "awr-operator-history-migration-v1";
const SAMPLE_LIMIT: usize = 50;

fn invalid() -> PgError {
    PgError::Protocol("invalid operator history migration request".into())
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HistoryKind {
    Session,
    Claim,
    Event,
    Execution,
}

impl HistoryKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Claim => "claim",
            Self::Event => "event",
            Self::Execution => "execution",
        }
    }
}

/// Pure classification used by plan assembly and unit tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HistoryDecision {
    Attribute {
        workstream_id: String,
        ownership_version: i64,
        coordinator_epoch: Option<String>,
    },
    Refuse {
        reason: &'static str,
    },
}

pub(crate) fn classify_session(
    work_id: &str,
    ownership: Option<&OwnershipBinding>,
) -> HistoryDecision {
    match ownership {
        Some(o) if !work_id.is_empty() => HistoryDecision::Attribute {
            workstream_id: o.workstream_id.clone(),
            ownership_version: o.ownership_version,
            coordinator_epoch: None,
        },
        None => HistoryDecision::Refuse {
            reason: "work_id_missing_from_current_ownership",
        },
        Some(_) => HistoryDecision::Refuse {
            reason: "work_id_missing_from_current_ownership",
        },
    }
}

pub(crate) fn classify_claim(
    state: &str,
    work_id: &str,
    ownership: Option<&OwnershipBinding>,
    project_epoch: &str,
) -> HistoryDecision {
    if state == "active" {
        return HistoryDecision::Refuse {
            reason: "active_unattributed_claim_requires_manual_recovery",
        };
    }
    match ownership {
        Some(o) if !work_id.is_empty() && !project_epoch.is_empty() => HistoryDecision::Attribute {
            workstream_id: o.workstream_id.clone(),
            ownership_version: o.ownership_version,
            coordinator_epoch: Some(project_epoch.into()),
        },
        _ => HistoryDecision::Refuse {
            reason: "work_id_missing_from_current_ownership",
        },
    }
}

pub(crate) fn classify_event(
    work_id: Option<&str>,
    ownership: Option<&OwnershipBinding>,
) -> HistoryDecision {
    match work_id {
        None | Some("") => HistoryDecision::Refuse {
            reason: "event_lacks_work_id",
        },
        Some(_) => match ownership {
            Some(o) => HistoryDecision::Attribute {
                workstream_id: o.workstream_id.clone(),
                ownership_version: o.ownership_version,
                coordinator_epoch: None,
            },
            None => HistoryDecision::Refuse {
                reason: "work_id_missing_from_current_ownership",
            },
        },
    }
}

pub(crate) fn classify_execution() -> HistoryDecision {
    // CHECK requires executor_client_id when attributed; inventing it would forge identity.
    // Use owner-only execution-attribution-* with a reviewed executor_client_id instead.
    HistoryDecision::Refuse {
        reason: "use_execution_attribution_protocol_for_reviewed_executor_client_id",
    }
}

fn decision_json(kind: HistoryKind, id: &str, work_id: Option<&str>, d: &HistoryDecision) -> Value {
    match d {
        HistoryDecision::Attribute {
            workstream_id,
            ownership_version,
            coordinator_epoch,
        } => {
            let mut m = Map::new();
            m.insert("kind".into(), json!(kind.as_str()));
            m.insert("id".into(), json!(id));
            if let Some(w) = work_id {
                m.insert("work_id".into(), json!(w));
            }
            m.insert("action".into(), json!("attribute"));
            m.insert("workstream_id".into(), json!(workstream_id));
            m.insert(
                "ownership_version".into(),
                json!(ownership_version.to_string()),
            );
            if let Some(epoch) = coordinator_epoch {
                m.insert("coordinator_epoch".into(), json!(epoch));
            }
            Value::Object(m)
        }
        HistoryDecision::Refuse { reason } => json!({
            "kind": kind.as_str(),
            "id": id,
            "work_id": work_id,
            "action": "refuse",
            "reason": reason
        }),
    }
}

pub struct OperatorHistory;

impl OperatorHistory {
    /// Dry-run inventory + classification. No writes.
    pub async fn preview(client: &mut Client, tenant: &str, project: &str) -> PgResult<Value> {
        if ![tenant, project].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let operator = require_owner_project(&tx, tenant, project, false).await?;
        let plan = build_plan(&tx, tenant, project, &operator).await?;
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
                "SELECT result_json FROM awr_team.history_migrations
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

    /// Apply the exact reviewed plan digests. Attributes only the safe subset.
    pub async fn apply(
        client: &mut Client,
        tenant: &str,
        project: &str,
        request: &str,
        expected_state: &str,
        expected_plan: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, request].iter().all(|s| identity(s))
            || !hex(expected_state)
            || !hex(expected_plan)
        {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, tenant, project, true).await?;
        let intent_hash = hash(&json!({
            "protocol": PROTOCOL,
            "tenant_id": tenant,
            "project_id": project,
            "expected_state": expected_state,
            "expected_plan": expected_plan
        }))?;
        if let Some(r) = tx
            .query_opt(
                "SELECT request_hash,result_json FROM awr_team.history_migrations
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
        let plan = build_plan(&tx, tenant, project, &operator).await?;
        if plan["state_digest"].as_str() != Some(expected_state)
            || plan["plan_digest"].as_str() != Some(expected_plan)
        {
            return Err(PgError::PreconditionsChanged);
        }
        let attributable = plan["attributable"].as_array().cloned().unwrap_or_default();
        let mut attributed = Vec::new();
        for item in &attributable {
            let kind = item["kind"].as_str().unwrap_or("");
            let id = item["id"].as_str().unwrap_or("");
            let workstream = item["workstream_id"].as_str().unwrap_or("");
            let ownership: i64 = item["ownership_version"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .ok_or_else(invalid)?;
            match kind {
                "session" => {
                    let n = tx
                        .execute(
                            "UPDATE awr_team.sessions SET workstream_id=$4,ownership_version=$5
                        WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                          AND workstream_id IS NULL AND ownership_version IS NULL",
                            &[&tenant, &project, &id, &workstream, &ownership],
                        )
                        .await?;
                    if n != 1 {
                        return Err(PgError::PreconditionsChanged);
                    }
                }
                "claim" => {
                    let epoch = item["coordinator_epoch"].as_str().ok_or_else(invalid)?;
                    let n = tx
                        .execute(
                            "UPDATE awr_team.claims
                        SET workstream_id=$4,ownership_version=$5,coordinator_epoch=$6
                        WHERE tenant_id=$1 AND project_id=$2 AND id=$3
                          AND workstream_id IS NULL AND ownership_version IS NULL
                          AND coordinator_epoch IS NULL AND state<>'active'",
                            &[&tenant, &project, &id, &workstream, &ownership, &epoch],
                        )
                        .await?;
                    if n != 1 {
                        return Err(PgError::PreconditionsChanged);
                    }
                }
                "event" => {
                    let n = tx
                        .execute(
                            "UPDATE awr_team.events SET workstream_id=$4
                        WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND workstream_id IS NULL",
                            &[&tenant, &project, &id, &workstream],
                        )
                        .await?;
                    if n != 1 {
                        return Err(PgError::PreconditionsChanged);
                    }
                }
                _ => return Err(invalid()),
            }
            attributed.push(item.clone());
        }
        // Never touch completion_receipts, evidence, trust, actors, or executions here.
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
            "state_digest": expected_state,
            "plan_digest": expected_plan,
            "project_revision": revision.to_string(),
            "attributed": attributed,
            "refused": plan["refused"],
            "counts": plan["counts"],
            "completion_receipts_modified": false,
            "executions_modified": false,
            "identity_forged": false,
            "execution_authorized": false,
            "automatic_resume": false,
            "state_basis": "at_commit"
        });
        tx.execute(
            "INSERT INTO awr_team.history_migrations(tenant_id,project_id,request_id,request_hash,operator_role,result_json)
            VALUES($1,$2,$3,$4,$5,$6)",
            &[&tenant, &project, &request, &intent_hash, &operator, &receipt],
        )
        .await?;
        let event = json!({
            "operator_role": operator,
            "request_id": request,
            "attributed_count": attributed.len(),
            "refused_count": plan["refused"].as_array().map(|a| a.len()).unwrap_or(0)
        });
        tx.execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
            VALUES($1,$2,$3,$4,0,'history.migrated',$5,$6)",
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
) -> PgResult<Value> {
    let p = tx
        .query_one(
            "SELECT status,coordinator_epoch,project_revision,active_snapshot_id
        FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &project],
        )
        .await?;
    let status: String = p.get(0);
    // Migration is only offered for an active enabled project. Frozen/importing/
    // degraded projects must be recovered through their own protocols first.
    if status != "active" {
        return Err(PgError::ProjectNotAvailable);
    }
    let epoch: String = p.get(1);
    let ownership = load_ownership(tx, tenant, project).await?;

    let mut attributable = Vec::new();
    let mut refused = Vec::new();
    let mut push = |kind: HistoryKind, id: String, work_id: Option<String>, d: HistoryDecision| {
        let item = decision_json(kind, &id, work_id.as_deref(), &d);
        match d {
            HistoryDecision::Attribute { .. } => attributable.push(item),
            HistoryDecision::Refuse { .. } => refused.push(item),
        }
    };

    let sessions = tx
        .query(
            "SELECT id,work_id FROM awr_team.sessions
        WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL
        ORDER BY id FOR SHARE",
            &[&tenant, &project],
        )
        .await?;
    for r in sessions {
        let id: String = r.get(0);
        let work_id: String = r.get(1);
        let d = classify_session(&work_id, ownership.get(&work_id));
        push(HistoryKind::Session, id, Some(work_id), d);
    }

    let claims = tx
        .query(
            "SELECT id,work_id,state FROM awr_team.claims
        WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL
        ORDER BY id FOR SHARE",
            &[&tenant, &project],
        )
        .await?;
    for r in claims {
        let id: String = r.get(0);
        let work_id: String = r.get(1);
        let state: String = r.get(2);
        let d = classify_claim(&state, &work_id, ownership.get(&work_id), &epoch);
        push(HistoryKind::Claim, id, Some(work_id), d);
    }

    let events = tx
        .query(
            "SELECT id,work_id FROM awr_team.events
        WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL
        ORDER BY id FOR SHARE",
            &[&tenant, &project],
        )
        .await?;
    for r in events {
        let id: String = r.get(0);
        let work_id: Option<String> = r.get(1);
        let d = classify_event(
            work_id.as_deref(),
            work_id.as_ref().and_then(|w| ownership.get(w)),
        );
        push(HistoryKind::Event, id, work_id, d);
    }

    let executions = tx
        .query(
            "SELECT id,work_id FROM awr_team.executions
        WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL
        ORDER BY id FOR SHARE",
            &[&tenant, &project],
        )
        .await?;
    for r in executions {
        let id: String = r.get(0);
        let work_id: String = r.get(1);
        push(
            HistoryKind::Execution,
            id,
            Some(work_id),
            classify_execution(),
        );
    }

    let counts = json!({
        "attributable": attributable.len(),
        "refused": refused.len(),
        "sessions_attributable": count_kind(&attributable, "session"),
        "claims_attributable": count_kind(&attributable, "claim"),
        "events_attributable": count_kind(&attributable, "event"),
        "executions_attributable": 0,
        "sessions_refused": count_kind(&refused, "session"),
        "claims_refused": count_kind(&refused, "claim"),
        "events_refused": count_kind(&refused, "event"),
        "executions_refused": count_kind(&refused, "execution")
    });

    let state = json!({
        "tenant_id": tenant,
        "project_id": project,
        "project_status": status,
        "coordinator_epoch": epoch,
        "project_revision": p.get::<_, i64>(2).to_string(),
        "source_snapshot_id": p.get::<_, Option<String>>(3),
        "ownership_work_ids": ownership.keys().cloned().collect::<Vec<_>>(),
        "unattributed": {
            "sessions": sessions_count(&attributable, &refused, "session"),
            "claims": sessions_count(&attributable, &refused, "claim"),
            "events": sessions_count(&attributable, &refused, "event"),
            "executions": sessions_count(&attributable, &refused, "execution")
        }
    });
    let attributable_sample: Vec<_> = attributable.iter().take(SAMPLE_LIMIT).cloned().collect();
    let refused_sample: Vec<_> = refused.iter().take(SAMPLE_LIMIT).cloned().collect();
    let plan_body = json!({
        "protocol": PROTOCOL,
        "protocol_version": 1,
        "read_only_preview": true,
        "mutation_on_preview": false,
        "operator_role": operator,
        "tenant_id": tenant,
        "project_id": project,
        "attribution_basis": "current_workstream_ownership",
        "safe_subset": ["session", "inactive_claim", "event_with_work_id"],
        "unsafe_excluded": ["execution", "active_claim", "completion_receipt", "evidence", "trust_grade"],
        "forges_identity": false,
        "local_file_access": "not_server_acl_or_confidentiality_sandbox",
        "frontend_filtering": false,
        "counts": counts,
        "attributable": attributable,
        "refused": refused,
        "attributable_sample": attributable_sample,
        "refused_sample": refused_sample
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
        "attributable": attributable,
        "refused": refused,
        "safe_subset": plan_body["safe_subset"],
        "unsafe_excluded": plan_body["unsafe_excluded"],
        "attribution_basis": "current_workstream_ownership",
        "next_action": "Review attributable/refused lists; apply with exact state_digest and plan_digest. Executions and active claims stay refused; use access quarantine-* for those."
    }))
}

fn count_kind(items: &[Value], kind: &str) -> usize {
    items.iter().filter(|i| i["kind"] == kind).count()
}

fn sessions_count(attr: &[Value], refused: &[Value], kind: &str) -> usize {
    count_kind(attr, kind) + count_kind(refused, kind)
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
    fn sessions_attribute_only_with_current_ownership() {
        let o = own("ws-1", 3);
        assert!(matches!(
            classify_session("work-a", Some(&o)),
            HistoryDecision::Attribute {
                ownership_version: 3,
                ..
            }
        ));
        assert_eq!(
            classify_session("missing", None),
            HistoryDecision::Refuse {
                reason: "work_id_missing_from_current_ownership"
            }
        );
    }

    #[test]
    fn active_claims_and_executions_are_refused() {
        let o = own("ws-1", 1);
        assert_eq!(
            classify_claim("active", "work-a", Some(&o), "epoch"),
            HistoryDecision::Refuse {
                reason: "active_unattributed_claim_requires_manual_recovery"
            }
        );
        assert!(matches!(
            classify_claim("released", "work-a", Some(&o), "epoch"),
            HistoryDecision::Attribute {
                coordinator_epoch: Some(_),
                ..
            }
        ));
        assert_eq!(
            classify_execution(),
            HistoryDecision::Refuse {
                reason: "use_execution_attribution_protocol_for_reviewed_executor_client_id"
            }
        );
    }

    #[test]
    fn events_without_work_id_are_refused() {
        assert_eq!(
            classify_event(None, None),
            HistoryDecision::Refuse {
                reason: "event_lacks_work_id"
            }
        );
        let o = own("ws-1", 1);
        assert!(matches!(
            classify_event(Some("work-a"), Some(&o)),
            HistoryDecision::Attribute { .. }
        ));
    }

    #[test]
    fn decision_json_never_claims_identity_forgery_fields_for_attribute() {
        let d = HistoryDecision::Attribute {
            workstream_id: "ws".into(),
            ownership_version: 2,
            coordinator_epoch: None,
        };
        let v = decision_json(HistoryKind::Session, "s1", Some("w1"), &d);
        assert_eq!(v["action"], "attribute");
        assert!(v.get("executor_client_id").is_none());
        assert!(v.get("actor_id").is_none());
        assert!(v.get("completion_receipt_id").is_none());
    }
}
