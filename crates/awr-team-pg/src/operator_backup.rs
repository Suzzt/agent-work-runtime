//! Schema-owner enabled-project backup export and guarded restore fencing.
//! Never reachable through client HTTP/MCP. Physical `pg_basebackup` remains
//! external. This slice records inspectable PG-side inventory manifests, plans
//! restore fencing, and applies only the protective epoch/fence subset when
//! digests match. It never forges completion receipts, never replays outbox,
//! never clears recovery blocks, and never row-rewrites history from a manifest.
use crate::operator_access::require_owner_project;
use crate::{PgError, PgResult};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio_postgres::{Client, Transaction};

const PROTOCOL: &str = "awr-operator-backup-v1";
const FORMAT: &str = "awr-team-enabled-backup-v1";

fn invalid() -> PgError {
    PgError::Protocol("invalid operator backup/restore request".into())
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
fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn digest_value(v: &Value) -> PgResult<String> {
    Ok(sha256_hex(
        awr_team::canonical_json(v)
            .map_err(|_| invalid())?
            .as_slice(),
    ))
}

/// Pure restore-safety classification used by plan assembly and unit tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RestoreDecision {
    ApplyProtectiveFence,
    Refuse { reason: &'static str },
}

/// Decide whether guarded apply may run. Never permits outbox replay, receipt
/// forgery, recovery-block clearing, or overwrite when inventory diverges.
pub(crate) fn classify_restore(
    schema_matches: bool,
    manifest_hash_ok: bool,
    inventory_matches: bool,
    outbox_replay: bool,
    clear_recovery_block: bool,
    forge_completion: bool,
    enabled_project: bool,
) -> RestoreDecision {
    if !enabled_project {
        return RestoreDecision::Refuse {
            reason: "enabled_workstreams_required",
        };
    }
    if outbox_replay {
        return RestoreDecision::Refuse {
            reason: "outbox_replay_forbidden",
        };
    }
    if clear_recovery_block {
        return RestoreDecision::Refuse {
            reason: "clear_recovery_block_forbidden",
        };
    }
    if forge_completion {
        return RestoreDecision::Refuse {
            reason: "completion_receipt_forgery_forbidden",
        };
    }
    if !schema_matches {
        return RestoreDecision::Refuse {
            reason: "schema_version_mismatch",
        };
    }
    if !manifest_hash_ok {
        return RestoreDecision::Refuse {
            reason: "manifest_hash_mismatch",
        };
    }
    if !inventory_matches {
        return RestoreDecision::Refuse {
            reason: "inventory_diverged_refuse_unsafe_overwrite",
        };
    }
    RestoreDecision::ApplyProtectiveFence
}

pub(crate) fn protective_actions() -> Vec<&'static str> {
    vec![
        "bump_coordinator_epoch",
        "mark_nonterminal_executions_unknown",
        "revoke_active_claims",
        "interrupt_active_sessions",
        "set_recovery_blocked_and_advance_fences",
        "mark_reserved_resources_unknown",
        "fail_pending_outbox",
        "revoke_active_tenant_credentials",
        "record_restore_run_and_fencing_barriers",
    ]
}

pub(crate) fn excluded_actions() -> Vec<&'static str> {
    vec![
        "row_rewrite_from_manifest",
        "physical_pg_basebackup",
        "outbox_replay",
        "clear_recovery_blocked",
        "forge_completion_receipts",
        "rewrite_evidence_or_trust",
        "bypass_schema_owner_auth",
        "http_mcp_client_access",
    ]
}

pub struct OperatorBackup;

impl OperatorBackup {
    /// List recorded logical backups for an enabled project (read-only).
    pub async fn list(client: &mut Client, tenant: &str, project: &str) -> PgResult<Value> {
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
        let rows = tx
            .query(
                "SELECT id,manifest_hash,coordinator_epoch,schema_version,
                    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')
            FROM awr_team.backups
            WHERE tenant_id=$1 AND project_id=$2
            ORDER BY created_at DESC, id DESC
            LIMIT 100",
                &[&tenant, &project],
            )
            .await?;
        let backups = rows
            .iter()
            .map(|r| {
                json!({
                    "backup_id": r.get::<_, String>(0),
                    "manifest_hash": r.get::<_, String>(1),
                    "coordinator_epoch": r.get::<_, String>(2),
                    "schema_version": r.get::<_, i32>(3),
                    "created_at": r.get::<_, String>(4)
                })
            })
            .collect::<Vec<_>>();
        tx.commit().await?;
        Ok(json!({
            "protocol": PROTOCOL,
            "read_only": true,
            "mutation": false,
            "operator_role": operator,
            "tenant_id": tenant,
            "project_id": project,
            "authorization": "schema_owner_postgresql_role",
            "workstreams_required": true,
            "backups": backups,
            "physical_backup": "external_pg_basebackup_not_performed_here",
            "local_file_access": "not_server_acl_or_confidentiality_sandbox"
        }))
    }

    /// Record a versioned inspectable inventory manifest (logical backup metadata).
    pub async fn export(
        client: &mut Client,
        tenant: &str,
        project: &str,
        request: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, request].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, tenant, project, true).await?;
        let intent_hash = hash(&json!({
            "protocol": PROTOCOL,
            "op": "export",
            "tenant_id": tenant,
            "project_id": project
        }))?;
        if let Some(r) = tx
            .query_opt(
                "SELECT request_hash,result_json FROM awr_team.backup_operations
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
        let inv = build_inventory(&tx, tenant, project).await?;
        let epoch = inv["coordinator_epoch"].as_str().unwrap_or("").to_string();
        let revision = inv["project_revision"].clone();
        let status = inv["project_status"].as_str().unwrap_or("").to_string();
        if !matches!(status.as_str(), "active" | "frozen" | "degraded") {
            return Err(PgError::ProjectNotAvailable);
        }
        let manifest = json!({
            "format": FORMAT,
            "protocol": PROTOCOL,
            "protocol_version": 1,
            "schema_version": crate::EXPECTED_SCHEMA_VERSION,
            "tenant_id": tenant,
            "project_id": project,
            "coordinator_epoch": epoch,
            "project_revision": revision,
            "inventory": inv,
            "limitations": {
                "physical_bytes": "not_included_use_external_pg_basebackup",
                "artifact_content": "digests_and_presence_only",
                "row_restore": "not_performed_by_this_protocol",
                "completion_receipts": "never_forged_or_rewritten"
            }
        });
        let manifest_hash = digest_value(&manifest)?;
        let backup_id = crate::tx::new_id();
        let artifact_digests = inv["artifact_digests"].clone();
        let source_digests = inv["source_digests"].clone();
        tx.execute(
            "INSERT INTO awr_team.backups(
                tenant_id,project_id,id,manifest_hash,coordinator_epoch,schema_version,
                artifact_digests_json,source_digests_json,manifest_json)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            &[
                &tenant,
                &project,
                &backup_id,
                &manifest_hash,
                &epoch,
                &crate::EXPECTED_SCHEMA_VERSION,
                &artifact_digests,
                &source_digests,
                &manifest,
            ],
        )
        .await?;
        let proj_rev: i64 = tx
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
            "op": "export",
            "request_id": request,
            "request_hash": intent_hash,
            "operator_role": operator,
            "tenant_id": tenant,
            "project_id": project,
            "backup_id": backup_id,
            "manifest_hash": manifest_hash,
            "coordinator_epoch": epoch,
            "project_revision": proj_rev.to_string(),
            "schema_version": crate::EXPECTED_SCHEMA_VERSION,
            "inventory_digest": inv["inventory_digest"],
            "workstreams_enabled": true,
            "physical_backup": "external_required",
            "completion_receipts_modified": false,
            "execution_authorized": false,
            "automatic_resume": false,
            "state_basis": "at_commit"
        });
        tx.execute(
            "INSERT INTO awr_team.backup_operations(
                tenant_id,project_id,request_id,request_hash,operator_role,op,result_json)
             VALUES($1,$2,$3,$4,$5,'export',$6)",
            &[&tenant, &project, &request, &intent_hash, &operator, &receipt],
        )
        .await?;
        let event = json!({
            "operator_role": operator,
            "request_id": request,
            "backup_id": backup_id,
            "manifest_hash": manifest_hash
        });
        tx.execute(
            "INSERT INTO awr_team.events(
                tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
             VALUES($1,$2,$3,$4,0,'backup.exported',$5,$6)",
            &[
                &tenant,
                &project,
                &crate::tx::new_id(),
                &proj_rev,
                &format!("operator:{operator}"),
                &event,
            ],
        )
        .await?;
        tx.commit().await?;
        Ok(json!({
            "replayed": false,
            "receipt": receipt,
            "manifest": manifest,
            "next_action": "Retain matching external pg_basebackup with this backup_id/manifest_hash; use restore-preview before restore-apply."
        }))
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
                "SELECT result_json,op FROM awr_team.backup_operations
            WHERE tenant_id=$1 AND project_id=$2 AND request_id=$3",
                &[&tenant, &project, &request],
            )
            .await?;
        let result = match row {
            Some(r) => json!({
                "outcome": "committed",
                "op": r.get::<_, String>(1),
                "receipt": r.get::<_, Value>(0)
            }),
            None => json!({"outcome": "unknown"}),
        };
        tx.commit().await?;
        Ok(result)
    }

    /// Dry-run restore plan. No writes.
    pub async fn restore_preview(
        client: &mut Client,
        tenant: &str,
        project: &str,
        backup_id: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, backup_id]
            .iter()
            .all(|s| identity(s))
        {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let operator = require_owner_project(&tx, tenant, project, false).await?;
        let plan = build_restore_plan(&tx, tenant, project, backup_id, &operator).await?;
        tx.commit().await?;
        Ok(plan)
    }

    /// Apply protective fencing only when preview digests still match.
    pub async fn restore_apply(
        client: &mut Client,
        tenant: &str,
        project: &str,
        backup_id: &str,
        request: &str,
        expected_state: &str,
        expected_plan: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, backup_id, request]
            .iter()
            .all(|s| identity(s))
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
            "op": "restore",
            "tenant_id": tenant,
            "project_id": project,
            "backup_id": backup_id,
            "expected_state": expected_state,
            "expected_plan": expected_plan
        }))?;
        if let Some(r) = tx
            .query_opt(
                "SELECT request_hash,result_json FROM awr_team.backup_operations
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
        let plan = build_restore_plan(&tx, tenant, project, backup_id, &operator).await?;
        if plan["state_digest"].as_str() != Some(expected_state)
            || plan["plan_digest"].as_str() != Some(expected_plan)
        {
            return Err(PgError::PreconditionsChanged);
        }
        if plan["decision"] != "apply_protective_fence" {
            return Err(PgError::RestoreIncomplete);
        }

        let old_epoch: String = tx
            .query_one(
                "SELECT coordinator_epoch FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
                &[&tenant, &project],
            )
            .await?
            .get(0);
        let new_epoch = format!("restored-{}", crate::tx::new_id());
        // Protective fencing only — never rewrite completion_receipts/evidence/trust.
        tx.execute(
            "UPDATE awr_team.executions
             SET state='unknown',
                 unknown_reason='restore requires resource reconciliation',
                 cancel_requested=TRUE
             WHERE tenant_id=$1 AND project_id=$2
               AND state IN ('prepared','queued','accepted','running')",
            &[&tenant, &project],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.claims SET state='revoked',lease_version=lease_version+1
             WHERE tenant_id=$1 AND project_id=$2 AND state='active'",
            &[&tenant, &project],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.sessions SET state='interrupted',session_version=session_version+1
             WHERE tenant_id=$1 AND project_id=$2 AND state='active'",
            &[&tenant, &project],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.work_runtime
             SET recovery_blocked=TRUE,last_fence=last_fence+1,work_version=work_version+1
             WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant, &project],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.resource_reservations SET state='unknown'
             WHERE tenant_id=$1 AND project_id=$2 AND state='reserved'",
            &[&tenant, &project],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.outbox SET state='failed'
             WHERE tenant_id=$1 AND project_id=$2 AND state IN ('pending','sending')",
            &[&tenant, &project],
        )
        .await?;
        // Credentials are tenant-scoped; revoke so restored epochs cannot reuse them.
        tx.execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp()
             WHERE tenant_id=$1 AND revoked_at IS NULL",
            &[&tenant],
        )
        .await?;
        let project_revision: i64 = tx
            .query_opt(
                "UPDATE awr_team.projects
             SET coordinator_epoch=$3, status='active',
                 project_revision=project_revision+1
             WHERE tenant_id=$1 AND id=$2 AND project_revision<9223372036854775807
             RETURNING project_revision",
                &[&tenant, &project, &new_epoch],
            )
            .await?
            .ok_or(PgError::PreconditionsChanged)?
            .get(0);

        let mut fencing_barriers: Vec<Value> = tx
            .query(
                "SELECT scope_id,work_id,last_fence FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 ORDER BY scope_id,work_id",
                &[&tenant, &project],
            )
            .await?
            .iter()
            .map(|r| {
                json!({
                    "coordinator_epoch": new_epoch,
                    "tenant_id": tenant,
                    "project_id": project,
                    "scope_id": r.get::<_, String>(0),
                    "work_id": r.get::<_, String>(1),
                    "fence": r.get::<_, i64>(2).to_string()
                })
            })
            .collect();
        if fencing_barriers.is_empty() {
            fencing_barriers.push(json!({
                "coordinator_epoch": new_epoch,
                "tenant_id": tenant,
                "project_id": project,
                "scope_id": "",
                "work_id": "",
                "fence": "0"
            }));
        }
        let restore_id = crate::tx::new_id();
        let report = json!({
            "old_epoch": old_epoch,
            "inventory_verified": true,
            "execution_recovery_required": true,
            "fencing_barriers": fencing_barriers,
            "protocol": PROTOCOL,
            "safe_subset": true,
            "completion_receipts_modified": false,
            "outbox_replayed": false,
            "recovery_blocked_cleared": false
        });
        tx.execute(
            "INSERT INTO awr_team.restore_runs(
                tenant_id,project_id,id,backup_id,new_epoch,outbox_replayed,state,report_json)
             VALUES($1,$2,$3,$4,$5,FALSE,'completed',$6)",
            &[&tenant, &project, &restore_id, &backup_id, &new_epoch, &report],
        )
        .await?;
        let receipt = json!({
            "protocol": PROTOCOL,
            "op": "restore",
            "request_id": request,
            "request_hash": intent_hash,
            "operator_role": operator,
            "tenant_id": tenant,
            "project_id": project,
            "backup_id": backup_id,
            "restore_id": restore_id,
            "old_epoch": old_epoch,
            "new_epoch": new_epoch,
            "state_digest": expected_state,
            "plan_digest": expected_plan,
            "project_revision": project_revision.to_string(),
            "fencing_barriers": fencing_barriers,
            "applied_actions": protective_actions(),
            "excluded_actions": excluded_actions(),
            "completion_receipts_modified": false,
            "outbox_replayed": false,
            "recovery_blocked_cleared": false,
            "identity_forged": false,
            "execution_authorized": false,
            "automatic_resume": false,
            "state_basis": "at_commit"
        });
        tx.execute(
            "INSERT INTO awr_team.backup_operations(
                tenant_id,project_id,request_id,request_hash,operator_role,op,result_json)
             VALUES($1,$2,$3,$4,$5,'restore',$6)",
            &[&tenant, &project, &request, &intent_hash, &operator, &receipt],
        )
        .await?;
        let event = json!({
            "operator_role": operator,
            "request_id": request,
            "backup_id": backup_id,
            "restore_id": restore_id,
            "old_epoch": old_epoch,
            "new_epoch": new_epoch
        });
        tx.execute(
            "INSERT INTO awr_team.events(
                tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
             VALUES($1,$2,$3,$4,0,'backup.restore_fenced',$5,$6)",
            &[
                &tenant,
                &project,
                &crate::tx::new_id(),
                &project_revision,
                &format!("operator:{operator}"),
                &event,
            ],
        )
        .await?;
        tx.commit().await?;
        Ok(json!({
            "replayed": false,
            "receipt": receipt,
            "next_action": "Install fencing barriers at resources; use authorized execution.reconcile. This apply never clears recovery_blocked or forges completions."
        }))
    }
}

async fn build_inventory(tx: &Transaction<'_>, tenant: &str, project: &str) -> PgResult<Value> {
    let p = tx
        .query_one(
            "SELECT status,coordinator_epoch,project_revision,active_snapshot_id
        FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &project],
        )
        .await?;
    let status: String = p.get(0);
    let epoch: String = p.get(1);
    let revision: i64 = p.get(2);
    let active: Option<String> = p.get(3);

    let catalog_digest: Option<String> = match &active {
        Some(snap) => {
            let row = tx
                .query_opt(
                    "SELECT catalog_json FROM awr_team.workstream_catalogs
                WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
                    &[&tenant, &project, snap],
                )
                .await?;
            match row {
                Some(r) => Some(digest_value(&r.get::<_, Value>(0))?),
                None => None,
            }
        }
        None => None,
    };

    let ownership_rows = tx
        .query(
            "SELECT work_id,workstream_id,ownership_version
        FROM awr_team.workstream_ownership
        WHERE tenant_id=$1 AND project_id=$2
        ORDER BY work_id FOR SHARE",
            &[&tenant, &project],
        )
        .await?;
    let ownership: Vec<Value> = ownership_rows
        .iter()
        .map(|r| {
            json!({
                "work_id": r.get::<_, String>(0),
                "workstream_id": r.get::<_, String>(1),
                "ownership_version": r.get::<_, i64>(2).to_string()
            })
        })
        .collect();
    let ownership_digest = digest_value(&json!(ownership))?;

    let source_rows = tx
        .query(
            "SELECT id,manifest_digest FROM awr_team.source_snapshots
        WHERE tenant_id=$1 AND project_id=$2 ORDER BY id",
            &[&tenant, &project],
        )
        .await?;
    let sources: Vec<Value> = source_rows
        .iter()
        .map(|r| json!({"id": r.get::<_, String>(0), "manifest_digest": r.get::<_, String>(1)}))
        .collect();
    let source_digests: Vec<String> = sources
        .iter()
        .filter_map(|s| s["manifest_digest"].as_str().map(str::to_owned))
        .collect();

    let artifact_rows = tx
        .query(
            "SELECT id,sha256,byte_length,state,
                (content IS NOT NULL) AS has_content
        FROM awr_team.artifacts
        WHERE tenant_id=$1 AND project_id=$2 ORDER BY id",
            &[&tenant, &project],
        )
        .await?;
    let artifacts: Vec<Value> = artifact_rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<_, String>(0),
                "sha256": r.get::<_, String>(1),
                "byte_length": r.get::<_, i64>(2),
                "state": r.get::<_, String>(3),
                "content_present": r.get::<_, bool>(4)
            })
        })
        .collect();
    let artifact_digests: Vec<String> = artifacts
        .iter()
        .filter_map(|a| a["sha256"].as_str().map(str::to_owned))
        .collect();

    let contract_count: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.work_contracts
        WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant, &project],
        )
        .await?
        .get(0);
    let edge_count: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.dependency_edges
        WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant, &project],
        )
        .await?
        .get(0);
    let session_count: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.sessions WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant, &project],
        )
        .await?
        .get(0);
    let claim_count: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant, &project],
        )
        .await?
        .get(0);
    let execution_count: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant, &project],
        )
        .await?
        .get(0);
    let completion_count: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.completion_receipts
        WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant, &project],
        )
        .await?
        .get(0);
    let recovery_blocked: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.work_runtime
        WHERE tenant_id=$1 AND project_id=$2 AND recovery_blocked",
            &[&tenant, &project],
        )
        .await?
        .get(0);

    let mut body = Map::new();
    body.insert("tenant_id".into(), json!(tenant));
    body.insert("project_id".into(), json!(project));
    body.insert("project_status".into(), json!(status));
    body.insert("coordinator_epoch".into(), json!(epoch));
    body.insert("project_revision".into(), json!(revision.to_string()));
    body.insert("active_snapshot_id".into(), json!(active));
    body.insert("catalog_digest".into(), json!(catalog_digest));
    body.insert("ownership_digest".into(), json!(ownership_digest));
    body.insert("ownership".into(), json!(ownership));
    body.insert("sources".into(), json!(sources));
    body.insert("source_digests".into(), json!(source_digests));
    body.insert("artifacts".into(), json!(artifacts));
    body.insert("artifact_digests".into(), json!(artifact_digests));
    body.insert(
        "counts".into(),
        json!({
            "contracts": contract_count,
            "edges": edge_count,
            "sessions": session_count,
            "claims": claim_count,
            "executions": execution_count,
            "completion_receipts": completion_count,
            "recovery_blocked_work": recovery_blocked,
            "workstream_ownership": ownership_rows.len()
        }),
    );
    let inventory = Value::Object(body);
    let inventory_digest = digest_value(&inventory)?;
    let mut with_digest = match inventory {
        Value::Object(mut m) => {
            m.insert("inventory_digest".into(), json!(inventory_digest));
            Value::Object(m)
        }
        other => other,
    };
    // Keep digest field stable relative to hashed body: recompute excluding itself.
    if let Value::Object(ref mut m) = with_digest {
        let mut for_hash = m.clone();
        for_hash.remove("inventory_digest");
        let d = digest_value(&Value::Object(for_hash))?;
        m.insert("inventory_digest".into(), json!(d));
    }
    Ok(with_digest)
}

async fn build_restore_plan(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    backup_id: &str,
    operator: &str,
) -> PgResult<Value> {
    let backup = tx
        .query_opt(
            "SELECT manifest_hash,schema_version,manifest_json,coordinator_epoch
        FROM awr_team.backups
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &backup_id],
        )
        .await?
        .ok_or(PgError::RestoreIncomplete)?;
    let stored_hash: String = backup.get(0);
    let schema_version: i32 = backup.get(1);
    let manifest: Option<Value> = backup.get(2);
    let backup_epoch: String = backup.get(3);

    let schema_matches = schema_version == crate::EXPECTED_SCHEMA_VERSION
        && manifest
            .as_ref()
            .and_then(|m| m.get("schema_version"))
            .and_then(|v| v.as_i64())
            .map(|v| v as i32)
            == Some(crate::EXPECTED_SCHEMA_VERSION);
    let manifest_hash_ok = manifest
        .as_ref()
        .map(|m| digest_value(m).ok() == Some(stored_hash.clone()))
        .unwrap_or(false);

    let current = build_inventory(tx, tenant, project).await?;
    let recorded_inv = manifest
        .as_ref()
        .and_then(|m| m.get("inventory"))
        .cloned();
    let inventory_matches = match (&recorded_inv, &current) {
        (Some(recorded), current) => {
            // Compare stable projection fields; ignore live revision drift only
            // for status/epoch when physical restore placed the backed-up image.
            let mut a = recorded.clone();
            let mut b = current.clone();
            if let (Value::Object(ref mut am), Value::Object(ref mut bm)) = (&mut a, &mut b) {
                for key in ["inventory_digest", "project_revision", "project_status"] {
                    am.remove(key);
                    bm.remove(key);
                }
            }
            digest_value(&a)? == digest_value(&b)?
                && recorded
                    .get("inventory_digest")
                    .and_then(|d| d.as_str())
                    .is_some()
        }
        _ => false,
    };

    let decision = classify_restore(
        schema_matches,
        manifest_hash_ok,
        inventory_matches,
        false,
        false,
        false,
        true,
    );
    let (decision_str, refuse_reason) = match &decision {
        RestoreDecision::ApplyProtectiveFence => ("apply_protective_fence", Value::Null),
        RestoreDecision::Refuse { reason } => ("refuse", json!(reason)),
    };

    let state = json!({
        "tenant_id": tenant,
        "project_id": project,
        "backup_id": backup_id,
        "backup_epoch": backup_epoch,
        "stored_manifest_hash": stored_hash,
        "schema_version": schema_version,
        "schema_matches": schema_matches,
        "manifest_hash_ok": manifest_hash_ok,
        "inventory_matches": inventory_matches,
        "current_inventory_digest": current.get("inventory_digest"),
        "recorded_inventory_digest": recorded_inv
            .as_ref()
            .and_then(|i| i.get("inventory_digest"))
            .cloned(),
        "current_epoch": current.get("coordinator_epoch"),
        "current_revision": current.get("project_revision")
    });
    let plan_body = json!({
        "protocol": PROTOCOL,
        "protocol_version": 1,
        "op": "restore",
        "decision": decision_str,
        "refuse_reason": refuse_reason,
        "safe_subset": protective_actions(),
        "unsafe_excluded": excluded_actions(),
        "outbox_replay": false,
        "clear_recovery_blocked": false,
        "forge_completion_receipts": false,
        "row_rewrite_from_manifest": false,
        "physical_restore": "must_already_be_present_via_external_pg_basebackup",
        "authorization": "schema_owner_postgresql_role",
        "local_file_access": "not_server_acl_or_confidentiality_sandbox"
    });
    let state_digest = hash(&state)?;
    let plan_digest = hash(&plan_body)?;
    Ok(json!({
        "protocol": PROTOCOL,
        "applied": false,
        "read_only": true,
        "operator_role": operator,
        "tenant_id": tenant,
        "project_id": project,
        "backup_id": backup_id,
        "decision": decision_str,
        "refuse_reason": refuse_reason,
        "state_digest": state_digest,
        "plan_digest": plan_digest,
        "state": state,
        "plan": plan_body,
        "safe_subset": protective_actions(),
        "unsafe_excluded": excluded_actions(),
        "next_action": if decision_str == "apply_protective_fence" {
            "Review fencing plan; apply with exact state_digest and plan_digest. Physical image must already match."
        } else {
            "Refused: fix inventory/schema/manifest mismatch or use external physical recovery. Apply will not overwrite."
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_refuses_outbox_replay_and_diverged_inventory() {
        assert_eq!(
            classify_restore(true, true, true, true, false, false, true),
            RestoreDecision::Refuse {
                reason: "outbox_replay_forbidden"
            }
        );
        assert_eq!(
            classify_restore(true, true, false, false, false, false, true),
            RestoreDecision::Refuse {
                reason: "inventory_diverged_refuse_unsafe_overwrite"
            }
        );
        assert_eq!(
            classify_restore(true, true, true, false, true, false, true),
            RestoreDecision::Refuse {
                reason: "clear_recovery_block_forbidden"
            }
        );
        assert_eq!(
            classify_restore(true, true, true, false, false, true, true),
            RestoreDecision::Refuse {
                reason: "completion_receipt_forgery_forbidden"
            }
        );
        assert_eq!(
            classify_restore(false, true, true, false, false, false, true),
            RestoreDecision::Refuse {
                reason: "schema_version_mismatch"
            }
        );
        assert_eq!(
            classify_restore(true, false, true, false, false, false, true),
            RestoreDecision::Refuse {
                reason: "manifest_hash_mismatch"
            }
        );
        assert_eq!(
            classify_restore(true, true, true, false, false, false, false),
            RestoreDecision::Refuse {
                reason: "enabled_workstreams_required"
            }
        );
        assert_eq!(
            classify_restore(true, true, true, false, false, false, true),
            RestoreDecision::ApplyProtectiveFence
        );
    }

    #[test]
    fn protective_subset_never_includes_forbidden_actions() {
        let safe = protective_actions();
        let excluded = excluded_actions();
        assert!(safe.contains(&"bump_coordinator_epoch"));
        assert!(safe.contains(&"set_recovery_blocked_and_advance_fences"));
        assert!(excluded.contains(&"outbox_replay"));
        assert!(excluded.contains(&"forge_completion_receipts"));
        assert!(excluded.contains(&"clear_recovery_blocked"));
        assert!(excluded.contains(&"row_rewrite_from_manifest"));
        assert!(excluded.contains(&"bypass_schema_owner_auth"));
        for a in &safe {
            assert!(!excluded.contains(a));
        }
    }

    #[test]
    fn identity_rejects_empty_and_controls() {
        assert!(identity("tenant-a"));
        assert!(!identity(""));
        assert!(!identity("bad\nid"));
        assert!(!identity(&"x".repeat(129)));
    }

    #[test]
    fn protocol_constants_are_stable() {
        assert_eq!(PROTOCOL, "awr-operator-backup-v1");
        assert_eq!(FORMAT, "awr-team-enabled-backup-v1");
    }
}
