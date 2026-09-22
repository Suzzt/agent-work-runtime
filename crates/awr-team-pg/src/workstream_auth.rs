use crate::{PgError, PgResult};
use awr_core::{Id, WorkstreamAccess, WorkstreamAction, WorkstreamCatalog, WorkstreamGrant};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tokio_postgres::Transaction;

/// Hash a high-entropy operator-issued bearer. No raw credential is stored.
/// Format: `awr1.<credential id>.<64 lowercase hexadecimal characters>`.
pub fn workstream_credential_hash(token: &str) -> PgResult<String> {
    token_id(token)?;
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(format!("awr-team-credential-v1:{token}"))
    ))
}

fn token_id(token: &str) -> PgResult<&str> {
    if token.len() > 199 {
        return Err(PgError::Forbidden);
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3
        || parts[0] != "awr1"
        || parts[1].is_empty()
        || parts[1].len() > 128
        || !parts[1]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        || parts[2].len() != 64
        || !parts[2]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(PgError::Forbidden);
    }
    Ok(parts[1])
}

pub(crate) struct ReaderAuthority {
    pub actor_id: String,
    pub client_id: String,
    pub actor_kind: String,
    pub execution_access: BTreeMap<Id, ExecutionAccess>,
    pub access: WorkstreamAccess,
    pub catalog: WorkstreamCatalog,
    pub snapshot: String,
    pub epoch: String,
    pub project_status: String,
    pub revision: i64,
    pub binding: String,
    pub grant_versions: BTreeMap<Id, i64>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ExecutionAccess {
    pub attest: bool,
    pub reconcile: bool,
}

/// Resolve every security fact in the action transaction. Row locks make a
/// concurrent disable/revocation linearize before or after the read, never in
/// its middle. No caller-supplied actor, client or grant is accepted as proof.
pub(crate) async fn authenticate(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    token: &str,
) -> PgResult<ReaderAuthority> {
    authenticate_inner(tx, tenant, project, token, false).await
}

pub(crate) async fn authenticate_writer(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    token: &str,
) -> PgResult<ReaderAuthority> {
    authenticate_inner(tx, tenant, project, token, true).await
}

async fn authenticate_inner(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    token: &str,
    write: bool,
) -> PgResult<ReaderAuthority> {
    let credential_id = token_id(token)?;
    let hash = workstream_credential_hash(token)?;
    crate::tx::bind_workstream_scope(tx, tenant, project).await?;
    let mode = tx
        .query_opt(
            "SELECT enabled FROM awr_team.workstream_modes
        WHERE tenant_id=$1 AND project_id=$2 FOR SHARE",
            &[&tenant, &project],
        )
        .await?
        .ok_or(PgError::Forbidden)?;
    // Lock the project before identity/policy records; source/admin protocols
    // use the same admission -> project order.
    // Writers keep the existing project serialization barrier until the
    // operation-read-set protocol replaces it. Never upgrade a shared lock.
    let project_query = if write {
        "SELECT active_snapshot_id,coordinator_epoch,project_revision,status FROM awr_team.projects
        WHERE tenant_id=$1 AND id=$2 FOR UPDATE"
    } else {
        "SELECT active_snapshot_id,coordinator_epoch,project_revision,status FROM awr_team.projects
        WHERE tenant_id=$1 AND id=$2 FOR SHARE"
    };
    let p = tx
        .query_opt(project_query, &[&tenant, &project])
        .await?
        .ok_or(PgError::Forbidden)?;
    let identity = tx.query_opt("SELECT c.actor_id,c.client_id,m.membership_version,m.role,a.kind
        FROM awr_team.credentials c
        JOIN awr_team.tenants t ON t.id=c.tenant_id
        JOIN awr_team.actors a ON a.tenant_id=c.tenant_id AND a.id=c.actor_id
        JOIN awr_team.project_memberships m ON m.tenant_id=c.tenant_id AND m.actor_id=c.actor_id AND m.project_id=$2
        WHERE c.tenant_id=$1 AND c.id=$3 AND c.secret_hash=$4
          AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>clock_timestamp())
          AND t.status='active' AND a.status='active'
        FOR SHARE OF t,a,c,m", &[&tenant,&project,&credential_id,&hash]).await?.ok_or(PgError::Forbidden)?;
    if !mode.get::<_, bool>(0) {
        return Err(PgError::Unsupported(
            "project has not enabled workstreams".into(),
        ));
    }
    let actor: String = identity.get(0);
    let client: String = identity.get(1);
    let membership: i64 = identity.get(2);
    let role: String = identity.get(3);
    let actor_kind: String = identity.get(4);
    let snapshot: String = p
        .get::<_, Option<String>>(0)
        .ok_or(PgError::InactiveCandidate)?;
    let row = tx
        .query_opt(
            "SELECT catalog_json FROM awr_team.workstream_catalogs
        WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3",
            &[&tenant, &project, &snapshot],
        )
        .await?
        .ok_or(PgError::InactiveCandidate)?;
    let catalog: WorkstreamCatalog =
        serde_json::from_value(row.get(0)).map_err(|_| PgError::SourceDivergence)?;
    catalog.validate()?;
    if catalog.project_id != project {
        return Err(PgError::SourceDivergence);
    }
    let rows = tx.query("SELECT workstream_id,authority_version,can_read,can_write,can_manage,grant_version,
        can_attest_execution,can_reconcile_execution
        FROM awr_team.workstream_grants WHERE tenant_id=$1 AND project_id=$2 AND actor_id=$3 AND client_id=$4 AND active
        ORDER BY workstream_id FOR SHARE", &[&tenant,&project,&actor,&client]).await?;
    let mut grants = Vec::new();
    let mut grant_versions = BTreeMap::new();
    let mut execution_access = BTreeMap::new();
    for row in rows {
        let id: Id = row
            .get::<_, String>(0)
            .parse()
            .map_err(|_| PgError::Forbidden)?;
        let authority: i64 = row.get(1);
        let write = row.get::<_, bool>(3) && role != "reader";
        let manage = row.get::<_, bool>(4) && role == "admin";
        execution_access.insert(
            id,
            ExecutionAccess {
                attest: write && actor_kind == "system" && row.get::<_, bool>(6),
                reconcile: write
                    && manage
                    && matches!(actor_kind.as_str(), "system" | "human")
                    && row.get::<_, bool>(7),
            },
        );
        grants.push(WorkstreamGrant {
            workstream_id: id,
            authority_version: authority.try_into().map_err(|_| PgError::Forbidden)?,
            read: row.get(2),
            write,
            manage,
        });
        grant_versions.insert(id, row.get(5));
    }
    let binding = awr_team::request_hash(
        &json!({"tenant":tenant,"project":project,"credential":credential_id,
        "actor":actor,"client":client,"actor_kind":actor_kind,"membership":membership,"role":role}),
    )
    .map_err(|_| PgError::Forbidden)?;
    let access = WorkstreamAccess {
        project_id: project.into(),
        subject: binding.clone(),
        grants,
    };
    access.validate()?;
    Ok(ReaderAuthority {
        actor_id: actor,
        client_id: client,
        actor_kind,
        execution_access,
        access,
        catalog,
        snapshot,
        epoch: p.get(1),
        revision: p.get(2),
        project_status: p.get(3),
        binding,
        grant_versions,
    })
}

/// Shared Team domain-entry authority for HTTP/MCP/PG command paths.
/// Fine-grained TMCP business actions plug into this gate later; this enum is
/// the workstream write-boundary contract (AWR-WS-014).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DomainAuthority {
    /// Explicit write grant that may preserve work while a stream is paused.
    WritePreserve,
    /// Explicit write grant on an active workstream (ordinary mutations).
    WriteActive,
    /// Trusted executor attestation (system actor + explicit grant).
    Attest,
    /// Operator reconciliation (manage + explicit reconcile grant).
    Reconcile,
}

/// When authorization is rechecked relative to idempotent replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandAuthPhase {
    /// Before mutation: permits exact receipt replay after credential checks.
    Admission,
    /// After replay miss: enforces stream activity and special execution grants.
    Effect,
}

/// Map a Team workstream command op to its domain authority. Unknown ops are
/// unsupported capabilities and must be refused by callers.
pub(crate) fn command_authority(op: &str) -> Option<DomainAuthority> {
    Some(match op {
        "session.checkpoint" | "session.end" | "claim.release" | "execution.cancel"
        | "execution.report" => DomainAuthority::WritePreserve,
        "session.start" | "claim.acquire" | "claim.renew" | "execution.prepare"
        | "execution.start" => DomainAuthority::WriteActive,
        "execution.attest" => DomainAuthority::Attest,
        "execution.reconcile" => DomainAuthority::Reconcile,
        _ => return None,
    })
}

fn has_write(auth: &ReaderAuthority, stream: Id) -> bool {
    auth.access
        .grants
        .iter()
        .any(|grant| grant.workstream_id == stream && grant.write)
}

/// Shared authorization used by Team PG command dispatch (HTTP/MCP call the same
/// store). Request bodies, tool names and reconnects never supply grants.
///
/// Admission always requires a current read grant and an explicit write bit so
/// readers cannot mutate through any variant. Effect adds active-stream or
/// attest/reconcile checks after idempotent replay so historical receipts remain
/// replayable for the original client even if the stream later pauses or a
/// special grant is revoked.
pub(crate) fn authorize_command(
    auth: &ReaderAuthority,
    stream: Id,
    op: &str,
    phase: CommandAuthPhase,
) -> PgResult<()> {
    let required = command_authority(op).ok_or_else(|| {
        PgError::Unsupported(format!("unsupported workstream command capability: {op}"))
    })?;
    auth.access
        .authorize(&auth.catalog, stream, WorkstreamAction::Read)?;
    if !has_write(auth, stream) {
        return Err(PgError::Forbidden);
    }
    if phase == CommandAuthPhase::Admission {
        return Ok(());
    }
    match required {
        DomainAuthority::WritePreserve => Ok(()),
        DomainAuthority::WriteActive => {
            auth.access
                .authorize(&auth.catalog, stream, WorkstreamAction::Write)?;
            Ok(())
        }
        DomainAuthority::Attest => {
            if !auth
                .execution_access
                .get(&stream)
                .is_some_and(|access| access.attest)
            {
                return Err(PgError::Forbidden);
            }
            Ok(())
        }
        DomainAuthority::Reconcile => {
            if !auth
                .execution_access
                .get(&stream)
                .is_some_and(|access| access.reconcile)
            {
                return Err(PgError::Forbidden);
            }
            Ok(())
        }
    }
}

/// Capability metadata that makes `scope=main` historical semantics, old-client
/// write refusal, and the local-file vs server-ACL boundary explicit. Callers
/// merge these into live capabilities responses; unsupported keys stay refused.
pub(crate) fn workstream_boundary_capabilities() -> serde_json::Value {
    json!({
        "scope_id": "main",
        "scope_main_semantics": "historical_team_rows_retain_scope_id_main_while_workstream_id_isolates",
        "old_client_write_boundary": "legacy_unscoped_team_entrypoints_refuse_enabled_projects",
        "authorization": "transactional_workstream_grants",
        "domain_entry_authorization": "shared_command_gate",
        "write_authorization_phases": ["admission_write_grant", "effect_active_or_special"],
        "unsupported_capabilities": "refused",
        "local_file_access": "not_server_acl_or_confidentiality_sandbox",
        "reference_runner_effects": "operator_local_bounded_files_require_explicit_attestation_grant",
        "frontend_filtering": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::{
        WORKSTREAM_CATALOG_VERSION, Workstream, WorkstreamCatalog, WorkstreamGrant,
        WorkstreamState,
    };

    fn id(value: u128) -> Id {
        Id::from(value)
    }

    fn catalog(state: WorkstreamState) -> WorkstreamCatalog {
        WorkstreamCatalog {
            version: WORKSTREAM_CATALOG_VERSION,
            project_id: "project".into(),
            legacy_default: Some(id(1)),
            workstreams: vec![Workstream {
                id: id(1),
                project_id: "project".into(),
                external_key: "api".into(),
                title: "api".into(),
                state,
                authority_version: 1,
                goal_keys: vec!["g".into()],
                acceptance_contracts: vec!["c".into()],
            }],
        }
    }

    fn authority(
        write: bool,
        manage: bool,
        attest: bool,
        reconcile: bool,
        state: WorkstreamState,
    ) -> ReaderAuthority {
        let stream = id(1);
        let mut execution_access = BTreeMap::new();
        execution_access.insert(
            stream,
            ExecutionAccess { attest, reconcile },
        );
        ReaderAuthority {
            actor_id: "actor".into(),
            client_id: "client".into(),
            actor_kind: "system".into(),
            execution_access,
            access: WorkstreamAccess {
                project_id: "project".into(),
                subject: "subject".into(),
                grants: vec![WorkstreamGrant {
                    workstream_id: stream,
                    authority_version: 1,
                    read: true,
                    write,
                    manage,
                }],
            },
            catalog: catalog(state),
            snapshot: "snap".into(),
            epoch: "epoch".into(),
            project_status: "active".into(),
            revision: 1,
            binding: "binding".into(),
            grant_versions: BTreeMap::from([(stream, 1)]),
        }
    }

    #[test]
    fn every_supported_command_maps_to_a_domain_authority() {
        for op in crate::workstream_command::COMMANDS {
            assert!(
                command_authority(op).is_some(),
                "missing authority mapping for {op}"
            );
        }
        assert_eq!(command_authority("planning.publish"), None);
        assert_eq!(command_authority("work.claim"), None);
    }

    #[test]
    fn admission_rejects_readers_and_unknown_capabilities() {
        let reader = authority(false, false, false, false, WorkstreamState::Active);
        assert!(matches!(
            authorize_command(
                &reader,
                id(1),
                "session.start",
                CommandAuthPhase::Admission
            ),
            Err(PgError::Forbidden)
        ));
        let writer = authority(true, false, false, false, WorkstreamState::Active);
        assert!(matches!(
            authorize_command(
                &writer,
                id(1),
                "planning.publish",
                CommandAuthPhase::Admission
            ),
            Err(PgError::Unsupported(_))
        ));
        assert!(authorize_command(
            &writer,
            id(1),
            "session.checkpoint",
            CommandAuthPhase::Admission
        )
        .is_ok());
    }

    #[test]
    fn effect_enforces_active_stream_and_special_grants_after_admission() {
        let paused_writer = authority(true, false, false, false, WorkstreamState::Paused);
        assert!(authorize_command(
            &paused_writer,
            id(1),
            "session.end",
            CommandAuthPhase::Effect
        )
        .is_ok());
        assert!(matches!(
            authorize_command(
                &paused_writer,
                id(1),
                "session.start",
                CommandAuthPhase::Effect
            ),
            Err(PgError::Workstream(awr_core::WorkstreamError::Inactive))
        ));

        let writer = authority(true, false, false, false, WorkstreamState::Active);
        assert!(matches!(
            authorize_command(&writer, id(1), "execution.attest", CommandAuthPhase::Effect),
            Err(PgError::Forbidden)
        ));
        assert!(matches!(
            authorize_command(
                &writer,
                id(1),
                "execution.reconcile",
                CommandAuthPhase::Effect
            ),
            Err(PgError::Forbidden)
        ));

        let attester = authority(true, false, true, false, WorkstreamState::Active);
        assert!(authorize_command(
            &attester,
            id(1),
            "execution.attest",
            CommandAuthPhase::Effect
        )
        .is_ok());

        let reconciler = authority(true, true, false, true, WorkstreamState::Active);
        assert!(authorize_command(
            &reconciler,
            id(1),
            "execution.reconcile",
            CommandAuthPhase::Effect
        )
        .is_ok());
    }

    #[test]
    fn boundary_capabilities_make_scope_main_and_local_file_limits_explicit() {
        let caps = workstream_boundary_capabilities();
        assert_eq!(caps["scope_id"], "main");
        assert_eq!(caps["frontend_filtering"], false);
        assert_eq!(caps["unsupported_capabilities"], "refused");
        assert_eq!(
            caps["local_file_access"],
            "not_server_acl_or_confidentiality_sandbox"
        );
        assert_eq!(
            caps["old_client_write_boundary"],
            "legacy_unscoped_team_entrypoints_refuse_enabled_projects"
        );
        assert_eq!(caps["domain_entry_authorization"], "shared_command_gate");
    }
}
