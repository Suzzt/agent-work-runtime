//! Schema-owner read-only recovery inspection for enabled workstream projects.
//! Never reachable through client HTTP/MCP. Does not restore, migrate, clear
//! recovery blocks, or inspect host filesystems (those are not a server ACL).
use crate::operator_access::require_owner_project;
use crate::{PgError, PgResult};
use serde_json::{Value, json};
use tokio_postgres::Client;

const SAMPLE_LIMIT: i64 = 20;
const PROTOCOL: &str = "awr-operator-recovery-inspect-v1";

fn invalid() -> PgError {
    PgError::Protocol("invalid operator recovery inspection request".into())
}

fn identity(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}

/// Pure report envelope. Unit-tested without PostgreSQL.
pub(crate) fn recovery_report(
    tenant: &str,
    project: &str,
    operator_role: &str,
    project_facts: Value,
    findings: Value,
) -> Value {
    json!({
        "protocol": PROTOCOL,
        "protocol_version": 1,
        "read_only": true,
        "mutation": false,
        "restore": false,
        "backup": false,
        "history_migration": false,
        "automatic_resume": false,
        "execution_authorized": false,
        "frontend_filtering": false,
        "local_filesystem_inspected": false,
        "local_file_access": "not_server_acl_or_confidentiality_sandbox",
        "authorization": "schema_owner_postgresql_role",
        "workstreams_required": true,
        "scope_id_semantics": "historical_team_rows_retain_scope_id_main_while_workstream_id_isolates",
        "tenant_id": tenant,
        "project_id": project,
        "operator_role": operator_role,
        "project": project_facts,
        "findings": findings,
        "next_action": "Review findings; use authorized execution.reconcile, history-migration for inactive unattributed rows, or quarantine-* for active claims / unattributed nonterminal executions. This inspection never mutates."
    })
}

pub struct OperatorRecovery;

impl OperatorRecovery {
    /// Owner-only diagnostics for an enabled workstream project. Shared locks only;
    /// no writes, restores, or grant changes.
    pub async fn inspect(client: &mut Client, tenant: &str, project: &str) -> PgResult<Value> {
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
        let p = tx
            .query_one(
                "SELECT status,coordinator_epoch,project_revision,active_snapshot_id
            FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
                &[&tenant, &project],
            )
            .await?;
        let project_facts = json!({
            "status": p.get::<_, String>(0),
            "coordinator_epoch": p.get::<_, String>(1),
            "project_revision": p.get::<_, i64>(2).to_string(),
            "source_snapshot_id": p.get::<_, Option<String>>(3),
            "workstreams_enabled": true
        });
        let epoch: String = p.get(1);

        let blocked = tx
            .query(
                "SELECT scope_id,work_id,state,work_version,last_fence
            FROM awr_team.work_runtime
            WHERE tenant_id=$1 AND project_id=$2 AND recovery_blocked
            ORDER BY scope_id,work_id LIMIT $3",
                &[&tenant, &project, &SAMPLE_LIMIT],
            )
            .await?;
        let blocked_count: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.work_runtime
            WHERE tenant_id=$1 AND project_id=$2 AND recovery_blocked",
                &[&tenant, &project],
            )
            .await?
            .get(0);

        let nonterminal = tx
            .query(
                "SELECT id,work_id,state,workstream_id,coordinator_epoch,execution_version
            FROM awr_team.executions
            WHERE tenant_id=$1 AND project_id=$2
              AND state NOT IN ('succeeded','failed','cancelled')
            ORDER BY id LIMIT $3",
                &[&tenant, &project, &SAMPLE_LIMIT],
            )
            .await?;
        let nonterminal_count: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.executions
            WHERE tenant_id=$1 AND project_id=$2
              AND state NOT IN ('succeeded','failed','cancelled')",
                &[&tenant, &project],
            )
            .await?
            .get(0);
        let unknown_count: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.executions
            WHERE tenant_id=$1 AND project_id=$2 AND state='unknown'",
                &[&tenant, &project],
            )
            .await?
            .get(0);

        let active_claims: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.claims
            WHERE tenant_id=$1 AND project_id=$2 AND state='active'",
                &[&tenant, &project],
            )
            .await?
            .get(0);
        let open_waits: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.wait_items
            WHERE tenant_id=$1 AND project_id=$2 AND state='open'",
                &[&tenant, &project],
            )
            .await?
            .get(0);

        let unattributed_sessions: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.sessions
            WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL",
                &[&tenant, &project],
            )
            .await?
            .get(0);
        let unattributed_claims: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.claims
            WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL",
                &[&tenant, &project],
            )
            .await?
            .get(0);
        let unattributed_executions: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.executions
            WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL",
                &[&tenant, &project],
            )
            .await?
            .get(0);
        let previous_epoch_executions: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.executions
            WHERE tenant_id=$1 AND project_id=$2
              AND coordinator_epoch IS NOT NULL AND coordinator_epoch<>$3
              AND state NOT IN ('succeeded','failed','cancelled')",
                &[&tenant, &project, &epoch],
            )
            .await?
            .get(0);

        let restore_runs: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.restore_runs
            WHERE tenant_id=$1 AND project_id=$2",
                &[&tenant, &project],
            )
            .await?
            .get(0);

        let findings = json!({
            "recovery_blocked_work": {
                "count": blocked_count,
                "sample": blocked.iter().map(|r| json!({
                    "scope_id": r.get::<_, String>(0),
                    "work_id": r.get::<_, String>(1),
                    "state": r.get::<_, String>(2),
                    "work_version": r.get::<_, i64>(3).to_string(),
                    "last_fence": r.get::<_, i64>(4).to_string()
                })).collect::<Vec<_>>()
            },
            "nonterminal_executions": {
                "count": nonterminal_count,
                "unknown_count": unknown_count,
                "sample": nonterminal.iter().map(|r| json!({
                    "execution_id": r.get::<_, String>(0),
                    "work_id": r.get::<_, String>(1),
                    "state": r.get::<_, String>(2),
                    "workstream_id": r.get::<_, Option<String>>(3),
                    "coordinator_epoch": r.get::<_, Option<String>>(4),
                    "execution_version": r.get::<_, i64>(5).to_string()
                })).collect::<Vec<_>>()
            },
            "active_claims": active_claims,
            "open_waits": open_waits,
            "unattributed_history": {
                "sessions_without_workstream": unattributed_sessions,
                "claims_without_workstream": unattributed_claims,
                "executions_without_workstream": unattributed_executions,
                "blocks_enablement_updates": unattributed_sessions > 0
                    || unattributed_claims > 0
                    || unattributed_executions > 0
            },
            "previous_epoch_nonterminal_executions": previous_epoch_executions,
            "recorded_restore_runs": restore_runs,
            "sample_limit": SAMPLE_LIMIT
        });

        let report = recovery_report(tenant, project, &operator, project_facts, findings);
        tx.commit().await?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_explicitly_read_only_and_non_acl() {
        let report = recovery_report(
            "tenant",
            "project",
            "owner",
            json!({"status":"active","workstreams_enabled":true}),
            json!({"active_claims":0}),
        );
        assert_eq!(report["protocol"], PROTOCOL);
        assert_eq!(report["read_only"], true);
        assert_eq!(report["mutation"], false);
        assert_eq!(report["restore"], false);
        assert_eq!(report["backup"], false);
        assert_eq!(report["history_migration"], false);
        assert_eq!(report["automatic_resume"], false);
        assert_eq!(report["execution_authorized"], false);
        assert_eq!(report["frontend_filtering"], false);
        assert_eq!(report["local_filesystem_inspected"], false);
        assert_eq!(
            report["local_file_access"],
            "not_server_acl_or_confidentiality_sandbox"
        );
        assert_eq!(report["authorization"], "schema_owner_postgresql_role");
        assert_eq!(report["workstreams_required"], true);
        assert!(
            report["next_action"]
                .as_str()
                .unwrap()
                .contains("never mutates")
        );
        assert!(
            report["next_action"]
                .as_str()
                .unwrap()
                .contains("quarantine-*")
        );
    }

    #[test]
    fn identity_rejects_empty_and_control_characters() {
        assert!(identity("tenant-a"));
        assert!(!identity(""));
        assert!(!identity("bad\nid"));
        assert!(!identity(&"x".repeat(129)));
    }
}
