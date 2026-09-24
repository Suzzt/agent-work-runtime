//! Schema-owner backup metadata, guarded fencing restore, and bounded rebuild
//! for enabled projects.
//! Physical PostgreSQL basebackup remains external. Legacy ImportStore backup/restore
//! already refuse enabled workstreams; this CLI is the explicit replacement slice.
//! Verified fencing never copies rows. A separate digest-gated rebuild can
//! materialize work_items inventory + workstream_ownership when the project is
//! fenced and ownership-empty (or already matching). Never forges completion
//! receipts, grants, actors, catalogs, contracts, or client HTTP/MCP authority.
use crate::operator_access::require_owner_project;
use crate::{PgError, PgResult};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_postgres::{Client, Transaction};

const PROTOCOL: &str = "awr-operator-enabled-backup-v1";
const BACKUP_FORMAT: &str = "awr-team-enabled-backup-v1";

fn invalid() -> PgError {
    PgError::Protocol("invalid operator enabled-project backup/restore request".into())
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
fn digest_value(v: &Value) -> String {
    sha256_hex(
        awr_team::canonical_json(v)
            .expect("JSON value is canonicalizable")
            .as_slice(),
    )
}

/// Pure restore planning over already-captured digests (unit-tested without PG).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestoreDecision {
    pub safe_to_apply: bool,
    pub mode: &'static str,
    pub refusals: Vec<&'static str>,
}

pub(crate) fn plan_restore(
    backup_format: &str,
    schema_ok: bool,
    workstream_projection_matches: bool,
    completion_receipts_match: bool,
    logical_inventory_matches: bool,
    artifacts_present: bool,
    unattributed_history: bool,
    replay_outbox: bool,
) -> RestoreDecision {
    let mut refusals = Vec::new();
    if backup_format != BACKUP_FORMAT {
        refusals.push("unsupported_backup_format");
    }
    if !schema_ok {
        refusals.push("schema_version_mismatch");
    }
    if !workstream_projection_matches {
        refusals.push("workstream_projection_drift");
    }
    if !completion_receipts_match {
        refusals.push("completion_receipt_divergence_refuses_rewrite");
    }
    if unattributed_history {
        refusals.push("unattributed_history_present");
    }
    if replay_outbox {
        refusals.push("outbox_replay_forbidden");
    }
    if !artifacts_present {
        refusals.push("physical_or_logical_artifacts_missing");
    }
    if !logical_inventory_matches {
        refusals.push("logical_inventory_mismatch");
    }
    let safe = refusals.is_empty();
    RestoreDecision {
        safe_to_apply: safe,
        mode: if safe { "verified_fencing" } else { "refused" },
        refusals,
    }
}

/// Pure rebuild planning over already-captured digests (unit-tested without PG).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RebuildDecision {
    pub safe_to_apply: bool,
    pub mode: &'static str,
    pub refusals: Vec<&'static str>,
    pub insert_ownership: usize,
    pub insert_work_items: usize,
}

pub(crate) fn plan_rebuild(
    backup_format: &str,
    schema_ok: bool,
    manifest_ownership_valid: bool,
    work_coverage_ok: bool,
    ownership_empty: bool,
    ownership_digest_matches: bool,
    work_key_conflicts: bool,
    has_active_claims: bool,
    has_active_sessions: bool,
    has_live_executions: bool,
    unattributed_history: bool,
    ownership_rows: usize,
    missing_work_items: usize,
) -> RebuildDecision {
    let mut refusals = Vec::new();
    if backup_format != BACKUP_FORMAT {
        refusals.push("unsupported_backup_format");
    }
    if !schema_ok {
        refusals.push("schema_version_mismatch");
    }
    if !manifest_ownership_valid {
        refusals.push("invalid_manifest_ownership");
    }
    if !work_coverage_ok {
        refusals.push("work_items_missing_for_ownership");
    }
    if work_key_conflicts {
        refusals.push("work_item_external_key_conflict");
    }
    if !ownership_empty && !ownership_digest_matches {
        refusals.push("dangerous_ownership_overwrite");
    }
    if has_active_claims {
        refusals.push("active_claims_present");
    }
    if has_active_sessions {
        refusals.push("active_sessions_present");
    }
    if has_live_executions {
        refusals.push("live_executions_present_fence_first");
    }
    if unattributed_history {
        refusals.push("unattributed_history_present");
    }
    let safe = refusals.is_empty();
    let mode = if !safe {
        "refused"
    } else if ownership_digest_matches {
        "already_materialized"
    } else {
        "ownership_materialize"
    };
    RebuildDecision {
        safe_to_apply: safe,
        mode,
        refusals,
        insert_ownership: if safe && !ownership_digest_matches {
            ownership_rows
        } else {
            0
        },
        insert_work_items: if safe { missing_work_items } else { 0 },
    }
}

pub(crate) fn backup_limits_document() -> Value {
    json!({
        "physical_basebackup": "external_operator_responsibility",
        "logical_manifest": "recorded_in_awr_team.backups",
        "restores_table_rows_from_manifest": false,
        "rebuild_from_manifest": {
            "subset": "work_inventory_and_ownership_when_empty_v1",
            "requires_fencing_quiet": true,
            "work_items_external_key_only": true,
            "workstream_ownership": true,
            "work_contracts": false,
            "workstream_catalogs": false,
            "workstream_snapshot_ownership": false,
            "completion_receipts": false,
            "grants_or_actors": false,
            "dangerous_overwrite": false
        },
        "rewrites_completion_receipts": false,
        "forges_credentials_or_grants": false,
        "outbox_replay": false,
        "local_file_access": "not_server_acl_or_confidentiality_sandbox",
        "frontend_filtering": false,
        "legacy_import_store": "refuses_enabled_workstream_projects",
        "requires_history_migration_first": true
    })
}

pub struct OperatorBackup;

impl OperatorBackup {
    /// Record a versioned enabled-project logical backup manifest (metadata + digests).
    pub async fn backup(client: &mut Client, tenant: &str, project: &str) -> PgResult<Value> {
        if ![tenant, project].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client.transaction().await?;
        let operator = require_owner_project(&tx, tenant, project, true).await?;
        let projection = workstream_projection(&tx, tenant, project).await?;
        if projection["project_status"] != "active" {
            return Err(PgError::ProjectNotAvailable);
        }
        if projection["unattributed_history"] == true {
            return Err(PgError::Unsupported(
                "unattributed history must be migrated before enabled-project backup".into(),
            ));
        }
        let logical = logical_inventory_digests(&tx, tenant, project).await?;
        let work_inventory = work_inventory_rows(&tx, tenant, project).await?;
        let receipts = completion_receipt_inventory(&tx, tenant, project).await?;
        let epoch = projection["coordinator_epoch"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let artifact_digests = logical["artifact_digests"].clone();
        let source_digests = logical["source_digests"].clone();
        let rebuildable = json!({
            "subset": "work_inventory_and_ownership_when_empty_v1",
            "ownership": projection["ownership"].clone(),
            "ownership_digest": projection["ownership_digest"].clone(),
            "work_inventory": work_inventory["items"].clone(),
            "work_inventory_digest": work_inventory["digest"].clone()
        });
        let manifest = json!({
            "format": BACKUP_FORMAT,
            "protocol": PROTOCOL,
            "protocol_version": 1,
            "schema_version": crate::EXPECTED_SCHEMA_VERSION,
            "tenant_id": tenant,
            "project_id": project,
            "epoch": epoch,
            "workstreams_enabled": true,
            "projection": projection,
            "logical": logical,
            "work_inventory": work_inventory,
            "rebuildable": rebuildable,
            "completion_receipts": receipts,
            "limits": backup_limits_document(),
            "operator_role": operator
        });
        let manifest_hash = digest_value(&manifest);
        let id = crate::tx::new_id();
        tx.execute(
            "INSERT INTO awr_team.backups(tenant_id,project_id,id,manifest_hash,coordinator_epoch,schema_version,artifact_digests_json,source_digests_json,manifest_json)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            &[
                &tenant,
                &project,
                &id,
                &manifest_hash,
                &epoch,
                &crate::EXPECTED_SCHEMA_VERSION,
                &artifact_digests,
                &source_digests,
                &manifest,
            ],
        )
        .await?;
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
        let event = json!({"backup_id":id,"manifest_hash":manifest_hash,"operator_role":operator});
        tx.execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
            VALUES($1,$2,$3,$4,0,'backup.enabled_recorded',$5,$6)",
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
        Ok(json!({
            "protocol": PROTOCOL,
            "backup_id": id,
            "manifest_hash": manifest_hash,
            "coordinator_epoch": epoch,
            "schema_version": crate::EXPECTED_SCHEMA_VERSION,
            "workstreams_enabled": true,
            "limits": backup_limits_document(),
            "next_action": "Retain an external physical basebackup bound to this manifest_hash before restore-preview."
        }))
    }

    pub async fn inspect(
        client: &mut Client,
        tenant: &str,
        project: &str,
        backup_id: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, backup_id].iter().all(|s| identity(s)) {
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
                "SELECT manifest_hash,schema_version,coordinator_epoch,manifest_json
            FROM awr_team.backups WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &backup_id],
            )
            .await?
            .ok_or(PgError::RestoreIncomplete)?;
        let manifest: Option<Value> = row.get(3);
        tx.commit().await?;
        Ok(json!({
            "protocol": PROTOCOL,
            "backup_id": backup_id,
            "manifest_hash": row.get::<_, String>(0),
            "schema_version": row.get::<_, i32>(1),
            "coordinator_epoch": row.get::<_, String>(2),
            "manifest": manifest,
            "read_only": true,
            "limits": backup_limits_document()
        }))
    }

    pub async fn restore_preview(
        client: &mut Client,
        tenant: &str,
        project: &str,
        backup_id: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, backup_id].iter().all(|s| identity(s)) {
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
            "op": "restore.apply",
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
        if plan["decision"]["safe_to_apply"] != true {
            return Err(PgError::Unsupported(
                "restore plan refused; resolve refusals and preview again".into(),
            ));
        }
        let old_epoch: String = tx
            .query_one(
                "SELECT coordinator_epoch FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
                &[&tenant, &project],
            )
            .await?
            .get(0);
        let new_epoch = format!("restored-{}", crate::tx::new_id());
        tx.execute(
            "UPDATE awr_team.executions SET state='unknown',unknown_reason='enabled restore requires resource reconciliation',cancel_requested=TRUE
            WHERE tenant_id=$1 AND project_id=$2 AND state IN ('prepared','queued','accepted','running')",
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
            "UPDATE awr_team.work_runtime SET recovery_blocked=TRUE,last_fence=last_fence+1,work_version=work_version+1
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
        tx.execute(
            "UPDATE awr_team.projects SET coordinator_epoch=$3,status='active'
            WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &project, &new_epoch],
        )
        .await?;
        let mut fencing: Vec<Value> = tx
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
                    "scope_id": r.get::<_, String>(0),
                    "work_id": r.get::<_, String>(1),
                    "fence": r.get::<_, i64>(2).to_string()
                })
            })
            .collect();
        // Empty projects still need a project-level barrier for post-backup effects.
        if fencing.is_empty() {
            fencing.push(json!({
                "coordinator_epoch": new_epoch,
                "scope_id": "",
                "work_id": "",
                "fence": "0"
            }));
        }
        let restore_id = crate::tx::new_id();
        let report = json!({
            "old_epoch": old_epoch,
            "new_epoch": new_epoch,
            "inventory_verified": true,
            "execution_recovery_required": true,
            "completion_receipts_modified": false,
            "credentials_revoked": false,
            "outbox_replayed": false,
            "fencing_barriers": fencing,
            "mode": "verified_fencing"
        });
        tx.execute(
            "INSERT INTO awr_team.restore_runs(tenant_id,project_id,id,backup_id,new_epoch,outbox_replayed,state,report_json)
            VALUES($1,$2,$3,$4,$5,FALSE,'completed',$6)",
            &[&tenant, &project, &restore_id, &backup_id, &new_epoch, &report],
        )
        .await?;
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
            "op": "restore.apply",
            "request_id": request,
            "request_hash": intent_hash,
            "operator_role": operator,
            "tenant_id": tenant,
            "project_id": project,
            "backup_id": backup_id,
            "restore_id": restore_id,
            "state_digest": expected_state,
            "plan_digest": expected_plan,
            "project_revision": revision.to_string(),
            "report": report,
            "completion_receipts_modified": false,
            "identity_forged": false,
            "execution_authorized": false,
            "automatic_resume": false
        });
        tx.execute(
            "INSERT INTO awr_team.backup_operations(tenant_id,project_id,request_id,request_hash,operator_role,op,result_json)
            VALUES($1,$2,$3,$4,$5,'restore.apply',$6)",
            &[&tenant, &project, &request, &intent_hash, &operator, &receipt],
        )
        .await?;
        let event = json!({
            "operator_role": operator,
            "request_id": request,
            "backup_id": backup_id,
            "restore_id": restore_id,
            "new_epoch": new_epoch
        });
        tx.execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
            VALUES($1,$2,$3,$4,0,'backup.enabled_restore_fenced',$5,$6)",
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

    pub async fn restore_outcome(
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
                "SELECT result_json FROM awr_team.backup_operations
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

    /// Digest-gated preview for bounded logical rebuild from a backup manifest.
    /// Safe subset: missing work_items (id+external_key) + empty ownership rows.
    pub async fn rebuild_preview(
        client: &mut Client,
        tenant: &str,
        project: &str,
        backup_id: &str,
    ) -> PgResult<Value> {
        if ![tenant, project, backup_id].iter().all(|s| identity(s)) {
            return Err(invalid());
        }
        crate::check_schema(client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let operator = require_owner_project(&tx, tenant, project, false).await?;
        let plan = build_rebuild_plan(&tx, tenant, project, backup_id, &operator).await?;
        tx.commit().await?;
        Ok(plan)
    }

    /// Apply bounded rebuild using exact preview digests. Never overwrites divergent
    /// ownership, never touches receipts/grants/actors/catalogs/contracts.
    pub async fn rebuild_apply(
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
            "op": "rebuild.apply",
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
        let plan = build_rebuild_plan(&tx, tenant, project, backup_id, &operator).await?;
        if plan["state_digest"].as_str() != Some(expected_state)
            || plan["plan_digest"].as_str() != Some(expected_plan)
        {
            return Err(PgError::PreconditionsChanged);
        }
        if plan["decision"]["safe_to_apply"] != true {
            return Err(PgError::Unsupported(
                "rebuild plan refused; resolve refusals and preview again".into(),
            ));
        }
        let mode = plan["decision"]["mode"].as_str().unwrap_or("refused");
        let mut inserted_work = 0i64;
        let mut inserted_ownership = 0i64;
        if mode == "ownership_materialize" || mode == "already_materialized" {
            let work_items = plan["materialize"]["work_items_to_insert"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for item in &work_items {
                let wid = item["work_id"].as_str().unwrap_or("");
                let key = item["external_key"].as_str().unwrap_or("");
                if wid.is_empty() || key.is_empty() || !identity(wid) || !identity(key) {
                    return Err(invalid());
                }
                let n = tx
                    .execute(
                        "INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
                    VALUES($1,$2,$3,$4)
                    ON CONFLICT (tenant_id,project_id,id) DO NOTHING",
                        &[&tenant, &project, &wid, &key],
                    )
                    .await?;
                inserted_work += n as i64;
            }
            if mode == "ownership_materialize" {
                let ownership = plan["materialize"]["ownership_to_insert"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for row in &ownership {
                    let wid = row["work_id"].as_str().unwrap_or("");
                    let sid = row["workstream_id"].as_str().unwrap_or("");
                    let ver = row["ownership_version"]
                        .as_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0);
                    if wid.is_empty()
                        || sid.is_empty()
                        || ver <= 0
                        || !identity(wid)
                        || !identity(sid)
                    {
                        return Err(invalid());
                    }
                    let n = tx
                        .execute(
                            "INSERT INTO awr_team.workstream_ownership
                        (tenant_id,project_id,work_id,workstream_id,ownership_version)
                        VALUES($1,$2,$3,$4,$5)
                        ON CONFLICT (tenant_id,project_id,work_id) DO NOTHING",
                            &[&tenant, &project, &wid, &sid, &ver],
                        )
                        .await?;
                    inserted_ownership += n as i64;
                }
            }
        }
        // Re-verify ownership digest matches backup after materialization.
        let current = workstream_projection(&tx, tenant, project).await?;
        let expected_own = plan["materialize"]["target_ownership_digest"]
            .as_str()
            .unwrap_or("");
        if current["ownership_digest"].as_str() != Some(expected_own) {
            return Err(PgError::PreconditionsChanged);
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
        let report = json!({
            "mode": mode,
            "inserted_work_items": inserted_work,
            "inserted_ownership_rows": inserted_ownership,
            "ownership_digest": current["ownership_digest"],
            "completion_receipts_modified": false,
            "grants_modified": false,
            "actors_forged": false,
            "catalogs_rewritten": false,
            "contracts_rewritten": false
        });
        let receipt = json!({
            "protocol": PROTOCOL,
            "op": "rebuild.apply",
            "request_id": request,
            "request_hash": intent_hash,
            "operator_role": operator,
            "tenant_id": tenant,
            "project_id": project,
            "backup_id": backup_id,
            "state_digest": expected_state,
            "plan_digest": expected_plan,
            "project_revision": revision.to_string(),
            "report": report,
            "completion_receipts_modified": false,
            "identity_forged": false,
            "execution_authorized": false,
            "automatic_resume": false
        });
        tx.execute(
            "INSERT INTO awr_team.backup_operations(tenant_id,project_id,request_id,request_hash,operator_role,op,result_json)
            VALUES($1,$2,$3,$4,$5,'rebuild.apply',$6)",
            &[&tenant, &project, &request, &intent_hash, &operator, &receipt],
        )
        .await?;
        let event = json!({
            "operator_role": operator,
            "request_id": request,
            "backup_id": backup_id,
            "mode": mode,
            "inserted_ownership_rows": inserted_ownership,
            "inserted_work_items": inserted_work
        });
        tx.execute(
            "INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,payload_json)
            VALUES($1,$2,$3,$4,0,'backup.enabled_rebuild_ownership',$5,$6)",
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

    pub async fn rebuild_outcome(
        client: &mut Client,
        tenant: &str,
        project: &str,
        request: &str,
    ) -> PgResult<Value> {
        // Same immutable backup_operations receipt surface as restore_outcome.
        Self::restore_outcome(client, tenant, project, request).await
    }
}

async fn workstream_projection(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<Value> {
    let p = tx
        .query_one(
            "SELECT status,coordinator_epoch,project_revision,active_snapshot_id
        FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &project],
        )
        .await?;
    let snapshot: Option<String> = p.get(3);
    let catalog = match &snapshot {
        Some(s) => tx
            .query_opt(
                "SELECT catalog_json FROM awr_team.workstream_catalogs
            WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
                &[&tenant, &project, s],
            )
            .await?
            .map(|r| r.get::<_, Value>(0)),
        None => None,
    };
    let catalog_digest = catalog.as_ref().map(digest_value);
    let ownership: Vec<Value> = tx
        .query(
            "SELECT work_id,workstream_id,ownership_version FROM awr_team.workstream_ownership
        WHERE tenant_id=$1 AND project_id=$2 ORDER BY work_id",
            &[&tenant, &project],
        )
        .await?
        .iter()
        .map(|r| {
            json!({
                "work_id": r.get::<_, String>(0),
                "workstream_id": r.get::<_, String>(1),
                "ownership_version": r.get::<_, i64>(2).to_string()
            })
        })
        .collect();
    let ownership_digest = digest_value(&json!(ownership));
    let unattributed = tx
        .query_one(
            "SELECT
            EXISTS(SELECT 1 FROM awr_team.sessions WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL)
         OR EXISTS(SELECT 1 FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL)
         OR EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND workstream_id IS NULL)",
            &[&tenant, &project],
        )
        .await?
        .get::<_, bool>(0);
    Ok(json!({
        "project_status": p.get::<_, String>(0),
        "coordinator_epoch": p.get::<_, String>(1),
        "project_revision": p.get::<_, i64>(2).to_string(),
        "source_snapshot_id": snapshot,
        "catalog_digest": catalog_digest,
        "ownership": ownership,
        "ownership_digest": ownership_digest,
        "unattributed_history": unattributed
    }))
}

async fn completion_receipt_inventory(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<Value> {
    let rows = tx
        .query(
            "SELECT id,work_id,contract_hash,result_digest FROM awr_team.completion_receipts
        WHERE tenant_id=$1 AND project_id=$2 ORDER BY id",
            &[&tenant, &project],
        )
        .await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<_, String>(0),
                "work_id": r.get::<_, String>(1),
                "contract_hash": r.get::<_, String>(2),
                "result_digest": r.get::<_, String>(3)
            })
        })
        .collect();
    Ok(json!({
        "items": items,
        "digest": digest_value(&json!(items))
    }))
}

async fn logical_inventory_digests(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<Value> {
    let sources: Vec<Value> = tx
        .query(
            "SELECT id,manifest_digest,parser_version FROM awr_team.source_snapshots
        WHERE tenant_id=$1 AND project_id=$2 ORDER BY id",
            &[&tenant, &project],
        )
        .await?
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<_, String>(0),
                "manifest_digest": r.get::<_, String>(1),
                "parser_version": r.get::<_, String>(2)
            })
        })
        .collect();
    let artifacts: Vec<Value> = tx
        .query(
            "SELECT id,sha256,byte_length,state,(content IS NOT NULL) AS present
        FROM awr_team.artifacts WHERE tenant_id=$1 AND project_id=$2 ORDER BY id",
            &[&tenant, &project],
        )
        .await?
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<_, String>(0),
                "sha256": r.get::<_, String>(1),
                "byte_length": r.get::<_, i64>(2),
                "state": r.get::<_, String>(3),
                "bytes_present": r.get::<_, bool>(4)
            })
        })
        .collect();
    let all_present = artifacts.iter().all(|a| {
        a["bytes_present"] == true && matches!(a["state"].as_str(), Some("finalized" | "retained"))
    });
    let source_digests: Vec<String> = sources
        .iter()
        .filter_map(|s| s["manifest_digest"].as_str().map(str::to_string))
        .collect();
    let artifact_digests: Vec<String> = artifacts
        .iter()
        .filter_map(|a| a["sha256"].as_str().map(str::to_string))
        .collect();
    Ok(json!({
        "sources": sources,
        "artifacts": artifacts,
        "source_digests": source_digests,
        "artifact_digests": artifact_digests,
        "artifacts_present": all_present,
        "digest": digest_value(&json!({"sources":sources,"artifacts":artifacts}))
    }))
}

async fn work_inventory_rows(tx: &Transaction<'_>, tenant: &str, project: &str) -> PgResult<Value> {
    let items: Vec<Value> = tx
        .query(
            "SELECT id,external_key FROM awr_team.work_items
        WHERE tenant_id=$1 AND project_id=$2 ORDER BY id",
            &[&tenant, &project],
        )
        .await?
        .iter()
        .map(|r| {
            json!({
                "work_id": r.get::<_, String>(0),
                "external_key": r.get::<_, String>(1)
            })
        })
        .collect();
    Ok(json!({
        "items": items,
        "digest": digest_value(&json!(items))
    }))
}

async fn fencing_quiet_flags(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
) -> PgResult<(bool, bool, bool)> {
    let row = tx
        .query_one(
            "SELECT
            EXISTS(SELECT 1 FROM awr_team.claims WHERE tenant_id=$1 AND project_id=$2 AND state='active'),
            EXISTS(SELECT 1 FROM awr_team.sessions WHERE tenant_id=$1 AND project_id=$2 AND state='active'),
            EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2
                AND state IN ('prepared','queued','accepted','running'))",
            &[&tenant, &project],
        )
        .await?;
    Ok((row.get(0), row.get(1), row.get(2)))
}

fn parse_manifest_ownership(manifest: &Value) -> PgResult<Vec<Value>> {
    let rows = manifest
        .pointer("/projection/ownership")
        .or_else(|| manifest.pointer("/rebuildable/ownership"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let wid = row.get("work_id").and_then(|v| v.as_str()).unwrap_or("");
        let sid = row
            .get("workstream_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ver = row
            .get("ownership_version")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !identity(wid) || !identity(sid) || ver.parse::<i64>().ok().filter(|v| *v > 0).is_none()
        {
            return Err(PgError::RestoreIncomplete);
        }
        out.push(json!({
            "work_id": wid,
            "workstream_id": sid,
            "ownership_version": ver
        }));
    }
    out.sort_by(|a, b| {
        a["work_id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["work_id"].as_str().unwrap_or(""))
    });
    Ok(out)
}

fn parse_manifest_work_inventory(manifest: &Value) -> Vec<Value> {
    manifest
        .pointer("/work_inventory/items")
        .or_else(|| manifest.pointer("/rebuildable/work_inventory"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

async fn build_rebuild_plan(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    backup_id: &str,
    operator: &str,
) -> PgResult<Value> {
    let row = tx
        .query_opt(
            "SELECT manifest_hash,schema_version,manifest_json FROM awr_team.backups
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR SHARE",
            &[&tenant, &project, &backup_id],
        )
        .await?
        .ok_or(PgError::RestoreIncomplete)?;
    let stored_hash: String = row.get(0);
    let schema_version: i32 = row.get(1);
    let manifest: Value = row
        .get::<_, Option<Value>>(2)
        .ok_or(PgError::RestoreIncomplete)?;
    if digest_value(&manifest) != stored_hash {
        return Err(PgError::RestoreIncomplete);
    }
    let format = manifest["format"].as_str().unwrap_or("");
    let ownership_present = manifest.pointer("/projection/ownership").is_some()
        || manifest.pointer("/rebuildable/ownership").is_some();
    let (ownership_rows, target_ownership_digest, manifest_ownership_valid) =
        match parse_manifest_ownership(&manifest) {
            Ok(rows) if ownership_present => {
                let digest = digest_value(&json!(rows));
                let bak_digest = manifest
                    .pointer("/projection/ownership_digest")
                    .or_else(|| manifest.pointer("/rebuildable/ownership_digest"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let digest_ok = bak_digest.is_empty() || bak_digest == digest;
                (rows, digest, digest_ok)
            }
            _ => (Vec::new(), String::new(), false),
        };

    let current = workstream_projection(tx, tenant, project).await?;
    let current_work = work_inventory_rows(tx, tenant, project).await?;
    let (active_claims, active_sessions, live_executions) =
        fencing_quiet_flags(tx, tenant, project).await?;

    let inventory = parse_manifest_work_inventory(&manifest);
    let mut current_by_id = std::collections::BTreeMap::new();
    if let Some(items) = current_work["items"].as_array() {
        for item in items {
            if let (Some(id), Some(key)) = (
                item.get("work_id").and_then(|v| v.as_str()),
                item.get("external_key").and_then(|v| v.as_str()),
            ) {
                current_by_id.insert(id.to_string(), key.to_string());
            }
        }
    }

    let mut work_key_conflicts = false;
    let mut inventory_by_id = std::collections::BTreeMap::new();
    for item in &inventory {
        let wid = item.get("work_id").and_then(|v| v.as_str()).unwrap_or("");
        let key = item
            .get("external_key")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !identity(wid) || !identity(key) {
            work_key_conflicts = true;
            continue;
        }
        if let Some(existing) = current_by_id.get(wid) {
            if existing != key {
                work_key_conflicts = true;
            }
        }
        inventory_by_id.insert(wid.to_string(), key.to_string());
    }

    // Coverage: every ownership work_id must exist now or be insertable from inventory.
    // Only ownership-referenced work_items are candidates for insert (bounded subset).
    let mut missing = 0usize;
    let mut work_items_to_insert = Vec::new();
    for row in &ownership_rows {
        let wid = row["work_id"].as_str().unwrap_or("");
        if current_by_id.contains_key(wid) {
            continue;
        }
        match inventory_by_id.get(wid) {
            Some(key) => {
                work_items_to_insert.push(json!({"work_id": wid, "external_key": key.as_str()}))
            }
            None => missing += 1,
        }
    }
    let work_coverage_ok = missing == 0;
    let ownership_empty = current["ownership"]
        .as_array()
        .map(|a| a.is_empty())
        .unwrap_or(true);
    let ownership_digest_matches =
        current["ownership_digest"].as_str() == Some(target_ownership_digest.as_str());

    let decision = plan_rebuild(
        format,
        schema_version == crate::EXPECTED_SCHEMA_VERSION
            && manifest["schema_version"] == crate::EXPECTED_SCHEMA_VERSION,
        manifest_ownership_valid,
        work_coverage_ok,
        ownership_empty,
        ownership_digest_matches,
        work_key_conflicts,
        active_claims,
        active_sessions,
        live_executions,
        current["unattributed_history"] == true,
        ownership_rows.len(),
        work_items_to_insert.len(),
    );

    let state = json!({
        "tenant_id": tenant,
        "project_id": project,
        "backup_id": backup_id,
        "backup_manifest_hash": stored_hash,
        "target_ownership_digest": target_ownership_digest,
        "current_ownership_digest": current["ownership_digest"],
        "current_work_inventory_digest": current_work["digest"],
        "active_claims": active_claims,
        "active_sessions": active_sessions,
        "live_executions": live_executions
    });
    let plan_body = json!({
        "protocol": PROTOCOL,
        "op": "rebuild.preview",
        "backup_id": backup_id,
        "decision": {
            "safe_to_apply": decision.safe_to_apply,
            "mode": decision.mode,
            "refusals": decision.refusals,
            "insert_ownership": decision.insert_ownership,
            "insert_work_items": decision.insert_work_items
        },
        "actions_if_applied": [
            "insert_missing_work_items_id_external_key",
            "insert_ownership_rows_when_empty"
        ],
        "actions_never": [
            "overwrite_divergent_ownership",
            "rewrite_completion_receipts",
            "forge_credentials_or_grants",
            "rewrite_catalogs_or_contracts",
            "replay_outbox",
            "authorize_execution_or_resume"
        ],
        "limits": backup_limits_document()
    });
    Ok(json!({
        "protocol": PROTOCOL,
        "applied": false,
        "operator_role": operator,
        "state_digest": hash(&state)?,
        "plan_digest": hash(&plan_body)?,
        "state": state,
        "decision": plan_body["decision"],
        "actions_if_applied": plan_body["actions_if_applied"],
        "actions_never": plan_body["actions_never"],
        "materialize": {
            "target_ownership_digest": target_ownership_digest,
            "ownership_to_insert": if decision.mode == "ownership_materialize" {
                ownership_rows
            } else {
                Vec::new()
            },
            "work_items_to_insert": if decision.safe_to_apply {
                work_items_to_insert
            } else {
                Vec::new()
            }
        },
        "limits": backup_limits_document(),
        "next_action": if decision.safe_to_apply {
            "Apply with exact state_digest and plan_digest after fencing quiet and inventory review."
        } else {
            "Resolve refusals (fence first / empty ownership / work coverage); do not apply."
        }
    }))
}

async fn build_restore_plan(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    backup_id: &str,
    operator: &str,
) -> PgResult<Value> {
    let row = tx
        .query_opt(
            "SELECT manifest_hash,schema_version,manifest_json FROM awr_team.backups
        WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR SHARE",
            &[&tenant, &project, &backup_id],
        )
        .await?
        .ok_or(PgError::RestoreIncomplete)?;
    let stored_hash: String = row.get(0);
    let schema_version: i32 = row.get(1);
    let manifest: Value = row
        .get::<_, Option<Value>>(2)
        .ok_or(PgError::RestoreIncomplete)?;
    if digest_value(&manifest) != stored_hash {
        return Err(PgError::RestoreIncomplete);
    }
    let format = manifest["format"].as_str().unwrap_or("");
    let current = workstream_projection(tx, tenant, project).await?;
    let current_receipts = completion_receipt_inventory(tx, tenant, project).await?;
    let current_logical = logical_inventory_digests(tx, tenant, project).await?;
    let bak_proj = &manifest["projection"];
    let workstream_ok = bak_proj["catalog_digest"] == current["catalog_digest"]
        && bak_proj["ownership_digest"] == current["ownership_digest"];
    let receipts_ok = manifest["completion_receipts"]["digest"] == current_receipts["digest"];
    let logical_ok = manifest["logical"]["digest"] == current_logical["digest"];
    let artifacts_present = current_logical["artifacts_present"] == true;
    let decision = plan_restore(
        format,
        schema_version == crate::EXPECTED_SCHEMA_VERSION
            && manifest["schema_version"] == crate::EXPECTED_SCHEMA_VERSION,
        workstream_ok,
        receipts_ok,
        logical_ok,
        artifacts_present,
        current["unattributed_history"] == true,
        false,
    );
    let state = json!({
        "tenant_id": tenant,
        "project_id": project,
        "backup_id": backup_id,
        "backup_manifest_hash": stored_hash,
        "current_projection": {
            "catalog_digest": current["catalog_digest"],
            "ownership_digest": current["ownership_digest"],
            "coordinator_epoch": current["coordinator_epoch"],
            "project_revision": current["project_revision"]
        },
        "current_completion_receipts_digest": current_receipts["digest"],
        "current_logical_digest": current_logical["digest"]
    });
    let plan_body = json!({
        "protocol": PROTOCOL,
        "op": "restore.preview",
        "backup_id": backup_id,
        "decision": {
            "safe_to_apply": decision.safe_to_apply,
            "mode": decision.mode,
            "refusals": decision.refusals
        },
        "actions_if_applied": [
            "rotate_coordinator_epoch",
            "interrupt_active_sessions",
            "revoke_active_claims",
            "mark_nonterminal_executions_unknown",
            "set_recovery_blocked_and_bump_fences",
            "fail_pending_outbox"
        ],
        "actions_never": [
            "rewrite_completion_receipts",
            "forge_credentials_or_grants",
            "replay_outbox",
            "copy_table_rows_from_manifest",
            "inspect_host_filesystem_as_acl"
        ],
        "limits": backup_limits_document()
    });
    Ok(json!({
        "protocol": PROTOCOL,
        "applied": false,
        "operator_role": operator,
        "state_digest": hash(&state)?,
        "plan_digest": hash(&plan_body)?,
        "state": state,
        "decision": plan_body["decision"],
        "actions_if_applied": plan_body["actions_if_applied"],
        "actions_never": plan_body["actions_never"],
        "limits": backup_limits_document(),
        "next_action": if decision.safe_to_apply {
            "Apply with exact state_digest and plan_digest after physical restore matches this inventory."
        } else {
            "Resolve refusals (projection/receipts/inventory/history); do not apply."
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_fencing_requires_all_digests_and_artifacts() {
        let ok = plan_restore(BACKUP_FORMAT, true, true, true, true, true, false, false);
        assert!(ok.safe_to_apply);
        assert_eq!(ok.mode, "verified_fencing");
    }

    #[test]
    fn completion_divergence_and_outbox_replay_are_refused() {
        let d = plan_restore(BACKUP_FORMAT, true, true, false, true, true, false, true);
        assert!(!d.safe_to_apply);
        assert!(
            d.refusals
                .contains(&"completion_receipt_divergence_refuses_rewrite")
        );
        assert!(d.refusals.contains(&"outbox_replay_forbidden"));
    }

    #[test]
    fn workstream_drift_and_unattributed_history_block_apply() {
        let d = plan_restore(BACKUP_FORMAT, true, false, true, true, true, true, false);
        assert!(!d.safe_to_apply);
        assert!(d.refusals.contains(&"workstream_projection_drift"));
        assert!(d.refusals.contains(&"unattributed_history_present"));
    }

    #[test]
    fn limits_document_rejects_acl_masquerade_and_receipt_rewrite() {
        let limits = backup_limits_document();
        assert_eq!(limits["rewrites_completion_receipts"], false);
        assert_eq!(limits["forges_credentials_or_grants"], false);
        assert_eq!(
            limits["local_file_access"],
            "not_server_acl_or_confidentiality_sandbox"
        );
        assert_eq!(
            limits["legacy_import_store"],
            "refuses_enabled_workstream_projects"
        );
        assert_eq!(limits["restores_table_rows_from_manifest"], false);
        assert_eq!(
            limits["rebuild_from_manifest"]["subset"],
            "work_inventory_and_ownership_when_empty_v1"
        );
        assert_eq!(
            limits["rebuild_from_manifest"]["dangerous_overwrite"],
            false
        );
        assert_eq!(
            limits["rebuild_from_manifest"]["completion_receipts"],
            false
        );
        assert_eq!(limits["rebuild_from_manifest"]["grants_or_actors"], false);
    }

    #[test]
    fn rebuild_materializes_when_fenced_and_ownership_empty() {
        let d = plan_rebuild(
            BACKUP_FORMAT,
            true,
            true,
            true,
            true,
            false,
            false,
            false,
            false,
            false,
            false,
            2,
            1,
        );
        assert!(d.safe_to_apply);
        assert_eq!(d.mode, "ownership_materialize");
        assert_eq!(d.insert_ownership, 2);
        assert_eq!(d.insert_work_items, 1);
    }

    #[test]
    fn rebuild_is_idempotent_when_ownership_digest_already_matches() {
        let d = plan_rebuild(
            BACKUP_FORMAT,
            true,
            true,
            true,
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            3,
            0,
        );
        assert!(d.safe_to_apply);
        assert_eq!(d.mode, "already_materialized");
        assert_eq!(d.insert_ownership, 0);
    }

    #[test]
    fn rebuild_refuses_dangerous_overwrite_and_live_activity() {
        let d = plan_rebuild(
            BACKUP_FORMAT,
            true,
            true,
            true,
            false,
            false,
            false,
            true,
            true,
            true,
            false,
            2,
            0,
        );
        assert!(!d.safe_to_apply);
        assert!(d.refusals.contains(&"dangerous_ownership_overwrite"));
        assert!(d.refusals.contains(&"active_claims_present"));
        assert!(d.refusals.contains(&"active_sessions_present"));
        assert!(d.refusals.contains(&"live_executions_present_fence_first"));
    }

    #[test]
    fn rebuild_refuses_missing_work_coverage_and_key_conflicts() {
        let d = plan_rebuild(
            BACKUP_FORMAT,
            true,
            true,
            false,
            true,
            false,
            true,
            false,
            false,
            false,
            false,
            1,
            0,
        );
        assert!(!d.safe_to_apply);
        assert!(d.refusals.contains(&"work_items_missing_for_ownership"));
        assert!(d.refusals.contains(&"work_item_external_key_conflict"));
    }
}
