//! Mainline evidence / review / rework / complete commands (WS-018).
use super::*;
use crate::review::{
    evidence_digest, required_dependencies_covered, resolve_person_id, self_review_permitted,
};
use awr_team::{CompletionView, EvidenceBundle, EvidenceGrade, ReviewPolicy, current_completion};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubmitEvidence {
    session_id: String,
    expected_session_version: String,
    claimed_trust: Option<String>,
    payload: Value,
    artifact_hex: Option<String>,
    input_digest: Option<String>,
    dirty_tree: bool,
    execution_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OpenReview {
    session_id: String,
    expected_session_version: String,
    evidence_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Decide {
    session_id: String,
    expected_session_version: String,
    round_id: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Rework {
    session_id: String,
    expected_session_version: String,
    round_id: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Complete {
    session_id: String,
    expected_session_version: String,
    evidence_id: String,
    requested_policy: Option<String>,
    context_complete: bool,
}

pub(super) enum Action {
    Submit(SubmitEvidence),
    Open(OpenReview),
    Accept(Decide),
    Return(Decide),
    Rework(Rework),
    Complete(Complete),
}

impl Action {
    pub(super) fn parse(op: &str, args: Value) -> PgResult<Self> {
        let action = match op {
            "evidence.submit" => Self::Submit(serde_json::from_value(args).map_err(|_| invalid())?),
            "review.open" => Self::Open(serde_json::from_value(args).map_err(|_| invalid())?),
            "review.accept" => Self::Accept(serde_json::from_value(args).map_err(|_| invalid())?),
            "review.return" => Self::Return(serde_json::from_value(args).map_err(|_| invalid())?),
            "work.rework" => Self::Rework(serde_json::from_value(args).map_err(|_| invalid())?),
            "work.complete" => Self::Complete(serde_json::from_value(args).map_err(|_| invalid())?),
            _ => return Err(invalid()),
        };
        let (s, v) = action.session();
        if !identity(s) || version(v)? == 0 {
            return Err(invalid());
        }
        Ok(action)
    }
    fn session(&self) -> (&str, &str) {
        match self {
            Self::Submit(a) => (&a.session_id, &a.expected_session_version),
            Self::Open(a) => (&a.session_id, &a.expected_session_version),
            Self::Accept(a) | Self::Return(a) => (&a.session_id, &a.expected_session_version),
            Self::Rework(a) => (&a.session_id, &a.expected_session_version),
            Self::Complete(a) => (&a.session_id, &a.expected_session_version),
        }
    }
    pub(super) fn requires_active_stream(&self) -> bool {
        !matches!(self, Self::Return(_) | Self::Rework(_))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn trust(actor_kind: &str, claimed: Option<&str>) -> String {
    match actor_kind {
        "system" => "trusted_executor".into(),
        "human" => "human_review".into(),
        _ => {
            let _ = claimed;
            "caller_asserted".into()
        }
    }
}

pub(super) async fn apply(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    ownership: i64,
    contract: &awr_team::WorkContract,
    action: Action,
) -> PgResult<Applied> {
    let (sid, ever) = action.session();
    session(tx, tenant, project, auth, command, sid, ever, ownership).await?;
    if action.requires_active_stream() {
        let ok: bool = tx
            .query_one(
                "SELECT c.definition_state='enabled' AND s.status='active'
                 FROM awr_team.work_contracts c
                 JOIN awr_team.work_scopes s
                   ON s.tenant_id=c.tenant_id AND s.project_id=c.project_id AND s.id=c.scope_id
                 WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.snapshot_id=$3
                   AND c.scope_id='main' AND c.work_id=$4",
                &[&tenant, &project, &auth.snapshot, &command.work_id],
            )
            .await?
            .get(0);
        if !ok {
            return Err(PgError::PreconditionsChanged);
        }
    }
    let contract_hash = contract
        .hash()
        .map_err(|e| PgError::Protocol(e.to_string()))?;
    match action {
        Action::Submit(a) => submit(tx, tenant, project, auth, command, &contract_hash, a).await,
        Action::Open(a) => open(tx, tenant, project, auth, command, a).await,
        Action::Accept(a) => {
            decide(
                tx,
                tenant,
                project,
                auth,
                command,
                &contract_hash,
                a,
                "approve",
            )
            .await
        }
        Action::Return(a) => {
            decide(
                tx,
                tenant,
                project,
                auth,
                command,
                &contract_hash,
                a,
                "reject",
            )
            .await
        }
        Action::Rework(a) => rework(tx, tenant, project, auth, command, a).await,
        Action::Complete(a) => {
            complete(
                tx,
                tenant,
                project,
                auth,
                command,
                contract,
                &contract_hash,
                a,
            )
            .await
        }
    }
}

async fn submit(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    contract_hash: &str,
    a: SubmitEvidence,
) -> PgResult<Applied> {
    let kind: String = tx
        .query_opt(
            "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &auth.actor_id],
        )
        .await?
        .map(|r| r.get(0))
        .ok_or(PgError::Forbidden)?;
    let trust_basis = trust(&kind, a.claimed_trust.as_deref());
    let artifact_bytes = match a.artifact_hex.as_deref() {
        None => None,
        Some(s) => {
            if s.len() > 2_097_152 || s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(invalid());
            }
            let mut out = Vec::with_capacity(s.len() / 2);
            let bytes = s.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                let hi = (bytes[i] as char).to_digit(16).ok_or_else(invalid)? as u8;
                let lo = (bytes[i + 1] as char).to_digit(16).ok_or_else(invalid)? as u8;
                out.push((hi << 4) | lo);
                i += 2;
            }
            if out.len() > 1_048_576 {
                return Err(invalid());
            }
            Some(out)
        }
    };
    if a.dirty_tree && a.input_digest.is_none() && artifact_bytes.is_none() {
        return Err(PgError::EvidenceInvalid);
    }
    if a.payload.get("passed").and_then(Value::as_bool) == Some(true)
        && artifact_bytes.is_none()
        && a.payload.get("output_digest").is_none()
    {
        return Err(PgError::EvidenceInvalid);
    }
    if let Some(d) = &a.input_digest {
        if !digest(d) {
            return Err(invalid());
        }
    }
    if let Some(id) = &a.execution_id {
        if !identity(id) {
            return Err(invalid());
        }
        let ok = tx
            .query_opt(
                "SELECT 1 FROM awr_team.executions
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND work_id=$4",
                &[&tenant, &project, id, &command.work_id],
            )
            .await?
            .is_some();
        if !ok {
            return Err(PgError::EvidenceInvalid);
        }
    }
    let output_digest = artifact_bytes.as_deref().map(sha256_hex);
    let execution_result_digest = a
        .payload
        .get("output_digest")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let digest_v = evidence_digest(
        &command.work_id,
        contract_hash,
        a.input_digest.as_deref(),
        output_digest.as_deref(),
        execution_result_digest.as_deref(),
        &a.payload,
    )?;
    let mut artifact_id = None;
    if let Some(bytes) = artifact_bytes.as_deref() {
        let id = crate::tx::new_id();
        tx.execute(
            "INSERT INTO awr_team.artifacts(
                tenant_id, project_id, id, object_key, sha256, byte_length,
                media_type, state, created_by, content)
             VALUES ($1,$2,$3,$4,$5,$6,'application/octet-stream','finalized',$7,$8)",
            &[
                &tenant,
                &project,
                &id,
                &format!("evidence/{id}"),
                &sha256_hex(bytes),
                &(bytes.len() as i64),
                &auth.actor_id,
                &bytes,
            ],
        )
        .await?;
        artifact_id = Some(id);
    }
    let id = crate::tx::new_id();
    tx.execute(
        "INSERT INTO awr_team.evidence(
            tenant_id, project_id, id, work_id, execution_id, artifact_id,
            contract_hash, input_digest, output_digest, execution_result_digest,
            evidence_kind, trust_basis, digest, payload_json, created_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'report',$11,$12,$13,$14)",
        &[
            &tenant,
            &project,
            &id,
            &command.work_id,
            &a.execution_id,
            &artifact_id,
            &contract_hash,
            &a.input_digest,
            &output_digest,
            &execution_result_digest,
            &trust_basis,
            &digest_v,
            &a.payload,
            &auth.actor_id,
        ],
    )
    .await?;
    Ok(Applied {
        data: json!({
            "evidence_id": id,
            "digest": digest_v,
            "trust_basis": trust_basis,
            "contract_hash": contract_hash,
            "execution_success": a.payload.get("passed").and_then(Value::as_bool),
            "author_self_report": trust_basis == "caller_asserted",
            "human_approval": false,
            "task_complete": false,
        }),
        preceding_events: vec![(
            "evidence.recorded",
            json!({
                "evidence_id": id,
                "digest": digest_v,
                "trust_basis": trust_basis,
                "execution_id": a.execution_id,
            }),
        )],
    })
}

async fn open(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    a: OpenReview,
) -> PgResult<Applied> {
    if !identity(&a.evidence_id) {
        return Err(invalid());
    }
    let row = tx
        .query_opt(
            "SELECT work_id, contract_hash, digest, execution_id, output_digest, execution_result_digest
             FROM awr_team.evidence WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &a.evidence_id],
        )
        .await?
        .ok_or(PgError::EvidenceInvalid)?;
    let work_id: String = row.get(0);
    let contract_hash: String = row.get(1);
    let digest_v: String = row.get(2);
    let execution_id: Option<String> = row.get(3);
    let artifact_digest: Option<String> = row.get(4);
    let execution_result_digest: Option<String> = row.get(5);
    if work_id != command.work_id {
        return Err(PgError::EvidenceInvalid);
    }
    // Resolve to a person for independence checks. Unbound agents get a
    // person row keyed by actor id so the FK holds; that person is not a
    // substitute for a real human owner when judging team independence.
    let author_person = match resolve_person_id(tx, tenant, project, &auth.actor_id).await {
        Ok(p) => p,
        Err(_) => {
            tx.execute(
                "INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
                 VALUES ($1,$2,$3,$3,'active') ON CONFLICT DO NOTHING",
                &[&tenant, &project, &auth.actor_id],
            )
            .await?;
            tx.execute(
                "INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
                 VALUES ($1,$2,$3,$4,$4,'active') ON CONFLICT DO NOTHING",
                &[&tenant, &project, &crate::tx::new_id(), &auth.actor_id],
            )
            .await?;
            auth.actor_id.clone()
        }
    };
    tx.execute(
        "UPDATE awr_team.review_rounds SET state='invalidated'
         WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
           AND state IN ('open','approved') AND bundle_hash <> $4",
        &[&tenant, &project, &command.work_id, &digest_v],
    )
    .await?;
    let round_index: i32 = tx
        .query_one(
            "SELECT COALESCE(max(round_index),0)+1 FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .get(0);
    let id = crate::tx::new_id();
    tx.execute(
        "INSERT INTO awr_team.review_rounds(
            tenant_id, project_id, id, work_id, round_index, bundle_hash,
            contract_hash, author_actor_id, state, author_person_id, evidence_id,
            execution_id, artifact_digest, execution_result_digest)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'open',$9,$10,$11,$12,$13)",
        &[
            &tenant,
            &project,
            &id,
            &command.work_id,
            &round_index,
            &digest_v,
            &contract_hash,
            &auth.actor_id,
            &author_person,
            &a.evidence_id,
            &execution_id,
            &artifact_digest,
            &execution_result_digest,
        ],
    )
    .await?;
    Ok(Applied {
        data: json!({
            "round_id": id,
            "round_index": round_index,
            "bundle_hash": digest_v,
            "contract_hash": contract_hash,
            "evidence_id": a.evidence_id,
            "execution_id": execution_id,
            "author_person_id": author_person,
            "state": "open",
            "binds_exact_contract_artifact_execution_round": true,
        }),
        preceding_events: vec![(
            "review.opened",
            json!({
                "round_id": id,
                "round_index": round_index,
                "bundle_hash": digest_v,
                "evidence_id": a.evidence_id,
                "author_person_id": author_person,
            }),
        )],
    })
}

async fn decide(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    contract_hash: &str,
    a: Decide,
    decision: &str,
) -> PgResult<Applied> {
    if !identity(&a.round_id) || a.reason.trim().is_empty() || a.reason.len() > 4096 {
        return Err(invalid());
    }
    let row = tx
        .query_opt(
            "SELECT work_id, author_actor_id, bundle_hash, state, round_index, contract_hash,
                    author_person_id, evidence_id
             FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
            &[&tenant, &project, &a.round_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("review round not found".into()))?;
    let work_id: String = row.get(0);
    let author: String = row.get(1);
    let bundle_hash: String = row.get(2);
    let state: String = row.get(3);
    let round_index: i32 = row.get(4);
    let round_contract: String = row.get(5);
    let author_person: Option<String> = row.get(6);
    let evidence_id: Option<String> = row.get(7);
    if work_id != command.work_id {
        return Err(PgError::Forbidden);
    }
    if state != "open" {
        return Err(PgError::ReviewRequired);
    }
    if round_contract != contract_hash {
        return Err(PgError::ReviewRequired);
    }
    let reviewer_kind: String = tx
        .query_opt(
            "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
            &[&tenant, &auth.actor_id],
        )
        .await?
        .map(|r| r.get(0))
        .ok_or(PgError::Forbidden)?;
    if reviewer_kind == "agent" {
        return Err(PgError::AuthorCannotReview);
    }
    crate::tx::validate_reviewer(tx, tenant, project, &auth.actor_id).await?;
    let author_person = match author_person {
        Some(p) => p,
        None => resolve_person_id(tx, tenant, project, &author)
            .await
            .unwrap_or(author.clone()),
    };
    let reviewer_person = resolve_person_id(tx, tenant, project, &auth.actor_id).await?;
    let same_person = author_person == reviewer_person || auth.actor_id == author;
    let live_policy: String = tx
        .query_opt(
            "SELECT contract_json->>'completion_policy'
             FROM awr_team.work_contracts
             WHERE tenant_id=$1 AND project_id=$2 AND snapshot_id=$3
               AND scope_id='main' AND work_id=$4",
            &[&tenant, &project, &auth.snapshot, &command.work_id],
        )
        .await?
        .and_then(|r| r.get::<_, Option<String>>(0))
        .unwrap_or_else(|| "trusted_execution_and_review".into());
    let independence_kind = if same_person {
        if decision == "approve" && !self_review_permitted(&live_policy) {
            return Err(PgError::AuthorCannotReview);
        }
        "personal_self_review"
    } else {
        "team_independent"
    };
    let next = if decision == "approve" {
        "approved"
    } else {
        "rejected"
    };
    tx.execute(
        "INSERT INTO awr_team.review_decisions(
            tenant_id, project_id, id, review_round_id, work_id, bundle_hash,
            reviewer_actor_id, decision, reason, reviewer_person_id, independence_kind)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        &[
            &tenant,
            &project,
            &crate::tx::new_id(),
            &a.round_id,
            &work_id,
            &bundle_hash,
            &auth.actor_id,
            &decision,
            &a.reason,
            &reviewer_person,
            &independence_kind,
        ],
    )
    .await?;
    tx.execute(
        "UPDATE awr_team.review_rounds SET state=$4
         WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
        &[&tenant, &project, &a.round_id, &next],
    )
    .await?;
    let team_independent_acceptance =
        independence_kind == "team_independent" && decision == "approve";
    Ok(Applied {
        data: json!({
            "round_id": a.round_id,
            "round_index": round_index,
            "state": next,
            "decision": decision,
            "independence_kind": independence_kind,
            "team_independent_acceptance": team_independent_acceptance,
            "author_person_id": author_person,
            "reviewer_person_id": reviewer_person,
            "evidence_id": evidence_id,
            "human_approval": decision == "approve",
            "task_complete": false,
        }),
        preceding_events: vec![(
            "review.decided",
            json!({
                "round_id": a.round_id,
                "decision": decision,
                "independence_kind": independence_kind,
                "team_independent_acceptance": team_independent_acceptance,
            }),
        )],
    })
}

async fn rework(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    a: Rework,
) -> PgResult<Applied> {
    if !identity(&a.round_id) || a.note.trim().is_empty() || a.note.len() > 4096 {
        return Err(invalid());
    }
    let row = tx
        .query_opt(
            "SELECT work_id, state, round_index FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
            &[&tenant, &project, &a.round_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("review round not found".into()))?;
    let work_id: String = row.get(0);
    let state: String = row.get(1);
    let round_index: i32 = row.get(2);
    if work_id != command.work_id {
        return Err(PgError::Forbidden);
    }
    if state != "rejected" {
        return Err(PgError::Protocol(
            "rework requires a returned/rejected review round".into(),
        ));
    }
    let actor_person = resolve_person_id(tx, tenant, project, &auth.actor_id)
        .await
        .unwrap_or_else(|_| auth.actor_id.clone());
    Ok(Applied {
        data: json!({
            "round_id": a.round_id,
            "round_index": round_index,
            "state": "rejected",
            "rework_acknowledged": true,
            "note": a.note,
            "actor_person_id": actor_person,
            "history_retained": true,
            "task_complete": false,
        }),
        preceding_events: vec![(
            "work.rework",
            json!({
                "round_id": a.round_id,
                "note": a.note,
                "history_retained": true,
            }),
        )],
    })
}

async fn complete(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    auth: &ReaderAuthority,
    command: &WorkstreamCommand,
    contract: &awr_team::WorkContract,
    contract_hash: &str,
    a: Complete,
) -> PgResult<Applied> {
    if !identity(&a.evidence_id) {
        return Err(invalid());
    }
    if !a.context_complete {
        return Err(PgError::ContextIncomplete);
    }
    let unknown: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.executions
             WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown'",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .get(0);
    if unknown > 0 {
        return Err(PgError::RecoveryBlocked);
    }
    let blocked: bool = tx
        .query_opt(
            "SELECT recovery_blocked FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .map(|r| r.get(0))
        .unwrap_or(false);
    if blocked {
        return Err(PgError::RecoveryBlocked);
    }
    crate::workstream_command::executions::require_clear_of_selective_blocks(
        tx,
        tenant,
        project,
        &command.work_id,
    )
    .await?;
    let policy = contract.completion_policy.as_str();
    if let Some(requested) = a.requested_policy.as_deref() {
        if requested != policy {
            return Err(PgError::PolicyDowngrade);
        }
    }
    // Delegate the heavy gates to ReviewStore-equivalent SQL via a nested
    // call pattern: reuse complete by constructing gates inline.
    // For WS-018 mainline, require an approved review round for this evidence
    // unless ordinary_confirm.
    let ev = tx
        .query_opt(
            "SELECT work_id, contract_hash, digest, trust_basis, payload_json, artifact_id,
                    output_digest, execution_result_digest, input_digest, execution_id, created_by
             FROM awr_team.evidence
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &a.evidence_id],
        )
        .await?
        .ok_or(PgError::EvidenceInvalid)?;
    let ev_work: String = ev.get(0);
    let ev_contract: String = ev.get(1);
    let ev_digest: String = ev.get(2);
    let trust_basis: String = ev.get(3);
    let payload: Value = ev.get(4);
    let artifact_id: Option<String> = ev.get(5);
    let output_digest: Option<String> = ev.get(6);
    let execution_result_digest: Option<String> = ev.get(7);
    let input_digest: Option<String> = ev.get(8);
    let execution_id: Option<String> = ev.get(9);
    let created_by: String = ev.get(10);
    if ev_work != command.work_id || ev_contract != contract_hash {
        return Err(PgError::EvidenceInvalid);
    }
    let recomputed = evidence_digest(
        &command.work_id,
        contract_hash,
        input_digest.as_deref(),
        output_digest.as_deref(),
        execution_result_digest.as_deref(),
        &payload,
    )?;
    if recomputed != ev_digest {
        return Err(PgError::EvidenceInvalid);
    }
    if let Some(aid) = &artifact_id {
        let row = tx
            .query_opt(
                "SELECT sha256, state, content FROM awr_team.artifacts
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, aid],
            )
            .await?
            .ok_or(PgError::EvidenceInvalid)?;
        let sha: String = row.get(0);
        let state: String = row.get(1);
        let content: Option<Vec<u8>> = row.get(2);
        let content = content.ok_or(PgError::EvidenceInvalid)?;
        if state != "finalized"
            || sha256_hex(&content) != sha
            || output_digest.as_deref() != Some(sha.as_str())
        {
            return Err(PgError::EvidenceInvalid);
        }
    }
    let grade = match trust_basis.as_str() {
        "trusted_executor" => EvidenceGrade::TrustedExecutionReceipt,
        "human_review" => EvidenceGrade::AuthorizedReview,
        _ => EvidenceGrade::AgentSelfReport,
    };
    let mut execution_success = false;
    if policy == "ordinary_confirm" {
        let kind: String = tx
            .query_one(
                "SELECT kind FROM awr_team.actors WHERE tenant_id=$1 AND id=$2",
                &[&tenant, &auth.actor_id],
            )
            .await?
            .get(0);
        if kind != "human" {
            return Err(PgError::Forbidden);
        }
    } else if grade != EvidenceGrade::TrustedExecutionReceipt {
        return Err(PgError::EvidenceInvalid);
    } else {
        if payload.get("passed").and_then(Value::as_bool) == Some(false) {
            return Err(PgError::EvidenceInvalid);
        }
        let execution_id = execution_id.as_deref().ok_or(PgError::EvidenceInvalid)?;
        let exec = tx
            .query_opt(
                "SELECT state, contract_hash, input_digest, executor_actor_id, scope_id, result_digest
                 FROM awr_team.executions
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &execution_id],
            )
            .await?
            .ok_or(PgError::EvidenceInvalid)?;
        let exec_state: String = exec.get(0);
        let exec_contract: String = exec.get(1);
        let exec_input: Option<String> = exec.get(2);
        let exec_executor: String = exec.get(3);
        let exec_scope: String = exec.get(4);
        let exec_result: Option<String> = exec.get(5);
        if exec_state != "succeeded" || exec_contract != ev_contract {
            return Err(PgError::EvidenceInvalid);
        }
        if exec_executor != created_by || exec_scope != "main" {
            return Err(PgError::EvidenceInvalid);
        }
        if exec_input != input_digest {
            return Err(PgError::EvidenceInvalid);
        }
        let declared = execution_result_digest
            .as_deref()
            .ok_or(PgError::EvidenceInvalid)?;
        let recorded = exec_result.as_deref().ok_or(PgError::EvidenceInvalid)?;
        if declared != recorded {
            return Err(PgError::EvidenceInvalid);
        }
        execution_success = true;
    }
    let contract_value =
        serde_json::to_value(contract).map_err(|e| PgError::Protocol(e.to_string()))?;
    let (binding_valid, dependency_links) = required_dependencies_covered(
        tx,
        tenant,
        project,
        &command.work_id,
        "main",
        &contract_value,
    )
    .await?;
    let mut review = ReviewPolicy {
        required: policy != "ordinary_confirm",
        author_may_self_approve: self_review_permitted(policy),
        approved: false,
        reviewer_is_author: false,
    };
    let mut independence_kind = if policy == "ordinary_confirm" {
        "ordinary_confirm".to_string()
    } else {
        "unspecified".to_string()
    };
    let mut approver_actor: Option<String> = None;
    let mut approver_person: Option<String> = None;
    if policy != "ordinary_confirm" {
        let pinned = tx
            .query_opt(
                "SELECT id, contract_hash FROM awr_team.review_rounds
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND bundle_hash=$4
                 ORDER BY round_index DESC LIMIT 1",
                &[&tenant, &project, &command.work_id, &ev_digest],
            )
            .await?
            .ok_or(PgError::ReviewRequired)?;
        let round_id: String = pinned.get(0);
        let round_contract: String = pinned.get(1);
        if round_contract != ev_contract {
            return Err(PgError::ReviewRequired);
        }
        let d = tx
            .query_opt(
                "SELECT decision, reviewer_actor_id, reviewer_person_id, independence_kind
                 FROM awr_team.review_decisions
                 WHERE tenant_id=$1 AND project_id=$2 AND review_round_id=$3
                 ORDER BY created_at DESC LIMIT 1",
                &[&tenant, &project, &round_id],
            )
            .await?
            .ok_or(PgError::ReviewRequired)?;
        let dec: String = d.get(0);
        approver_actor = Some(d.get(1));
        approver_person = d.get(2);
        independence_kind = d.get(3);
        review.approved = dec == "approve";
        if let Some(ap) = tx
            .query_opt(
                "SELECT author_person_id FROM awr_team.review_rounds
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant, &project, &round_id],
            )
            .await?
            .and_then(|r| r.get::<_, Option<String>>(0))
        {
            if let Some(rp) = &approver_person {
                review.reviewer_is_author = &ap == rp;
            }
        }
        if !review.approved {
            return Err(PgError::ReviewRequired);
        }
    }
    let bundle = EvidenceBundle {
        grade,
        contract_hash: ev_contract.clone(),
        artifact_digest: output_digest.clone(),
        accessible: true,
    };
    let view = current_completion(
        false,
        true,
        Some(ev_contract.as_str()),
        &ev_contract,
        binding_valid,
        Some(&bundle),
        &review,
    );
    if view != CompletionView::CurrentlyVerified {
        return Err(PgError::CompletionRejected);
    }
    let team_independent_acceptance = independence_kind == "team_independent";
    let submitter_person = resolve_person_id(tx, tenant, project, &auth.actor_id)
        .await
        .ok();
    let approved_by = json!({
        "approved_by": approver_actor,
        "approved_by_person_id": approver_person,
        "submitted_by": auth.actor_id,
        "submitted_by_person_id": submitter_person,
        "independence_kind": independence_kind,
        "team_independent_acceptance": team_independent_acceptance,
        "execution_success": execution_success,
        "author_self_report": trust_basis == "caller_asserted",
        "human_approval": review.approved,
    });
    let dependency_binding_hash = sha256_hex(json!(&dependency_links).to_string().as_bytes());
    let receipt_id = crate::tx::new_id();
    let independence_for_receipt = if policy == "ordinary_confirm" {
        "ordinary_confirm"
    } else {
        independence_kind.as_str()
    };
    tx.execute(
        "INSERT INTO awr_team.completion_receipts(
            tenant_id, project_id, id, work_id, scope_id, contract_hash,
            result_digest, dependency_binding_hash, evidence_bundle_hash,
            policy, approved_by_json, independence_kind, evidence_id, execution_id,
            approved_by_person_id, submitted_by_person_id)
         VALUES ($1,$2,$3,$4,'main',$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
        &[
            &tenant,
            &project,
            &receipt_id,
            &command.work_id,
            &ev_contract,
            &ev_digest,
            &dependency_binding_hash,
            &ev_digest,
            &policy,
            &approved_by,
            &independence_for_receipt,
            &a.evidence_id,
            &execution_id,
            &approver_person,
            &submitter_person,
        ],
    )
    .await?;
    tx.execute(
        "INSERT INTO awr_team.completion_evidence(
            tenant_id, project_id, completion_id, evidence_id, criterion_id)
         VALUES ($1,$2,$3,$4,'contract')",
        &[&tenant, &project, &receipt_id, &a.evidence_id],
    )
    .await?;
    for (upstream_work, upstream_receipt) in &dependency_links {
        tx.execute(
            "INSERT INTO awr_team.completion_dependencies(
                tenant_id, project_id, completion_id, predecessor_work_id,
                predecessor_completion_id)
             VALUES ($1,$2,$3,$4,$5)",
            &[
                &tenant,
                &project,
                &receipt_id,
                upstream_work,
                upstream_receipt,
            ],
        )
        .await?;
    }
    tx.execute(
        "INSERT INTO awr_team.work_runtime(
            tenant_id, project_id, scope_id, work_id, state, work_version, last_fence,
            selected_completion_id)
         VALUES ($1,$2,'main',$3,'completed',1,0,$4)
         ON CONFLICT (tenant_id, project_id, scope_id, work_id)
         DO UPDATE SET state='completed', selected_completion_id=$4,
             work_version = awr_team.work_runtime.work_version + 1",
        &[&tenant, &project, &command.work_id, &receipt_id],
    )
    .await?;
    let selected: Option<String> = tx
        .query_one(
            "SELECT selected_completion_id FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &command.work_id],
        )
        .await?
        .get(0);
    if selected.as_deref() != Some(receipt_id.as_str()) {
        return Err(PgError::CompletionRejected);
    }
    Ok(Applied {
        data: json!({
            "receipt_id": receipt_id,
            "selected_completion_id": receipt_id,
            "contract_hash": ev_contract,
            "policy": policy,
            "independence_kind": independence_for_receipt,
            "team_independent_acceptance": team_independent_acceptance,
            "evidence_id": a.evidence_id,
            "execution_id": execution_id,
            "approved_by_person_id": approver_person,
            "execution_success": execution_success,
            "author_self_report": trust_basis == "caller_asserted",
            "human_approval": review.approved,
            "task_complete": true,
            "provider_private_session": Value::Null,
        }),
        preceding_events: vec![(
            "work.completed",
            json!({
                "receipt_id": receipt_id,
                "evidence_id": a.evidence_id,
                "execution_id": execution_id,
                "independence_kind": independence_for_receipt,
                "team_independent_acceptance": team_independent_acceptance,
                "approved_by_person_id": approver_person,
            }),
        )],
    })
}

pub(crate) async fn inspect_evidence(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    evidence_id: &str,
) -> PgResult<Value> {
    let row = tx
        .query_opt(
            "SELECT id, work_id, contract_hash, digest, trust_basis, execution_id,
                    output_digest, execution_result_digest, created_by
             FROM awr_team.evidence
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &evidence_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("evidence not found".into()))?;
    Ok(json!({"evidence":{
        "evidence_id": row.get::<_,String>(0),
        "work_id": row.get::<_,String>(1),
        "contract_hash": row.get::<_,String>(2),
        "digest": row.get::<_,String>(3),
        "trust_basis": row.get::<_,String>(4),
        "execution_id": row.get::<_,Option<String>>(5),
        "artifact_digest": row.get::<_,Option<String>>(6),
        "execution_result_digest": row.get::<_,Option<String>>(7),
        "created_by": row.get::<_,String>(8),
    }}))
}

pub(crate) async fn inspect_review(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    round_id: &str,
) -> PgResult<Value> {
    let row = tx
        .query_opt(
            "SELECT id, work_id, round_index, bundle_hash, contract_hash, state,
                    author_actor_id, author_person_id, evidence_id, execution_id,
                    artifact_digest, execution_result_digest
             FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &round_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("review round not found".into()))?;
    let decisions = tx
        .query(
            "SELECT decision, reviewer_actor_id, reviewer_person_id, independence_kind, reason
             FROM awr_team.review_decisions
             WHERE tenant_id=$1 AND project_id=$2 AND review_round_id=$3
             ORDER BY created_at ASC",
            &[&tenant, &project, &round_id],
        )
        .await?;
    Ok(json!({"review":{
        "round_id": row.get::<_,String>(0),
        "work_id": row.get::<_,String>(1),
        "round_index": row.get::<_,i32>(2),
        "bundle_hash": row.get::<_,String>(3),
        "contract_hash": row.get::<_,String>(4),
        "state": row.get::<_,String>(5),
        "author_actor_id": row.get::<_,String>(6),
        "author_person_id": row.get::<_,Option<String>>(7),
        "evidence_id": row.get::<_,Option<String>>(8),
        "execution_id": row.get::<_,Option<String>>(9),
        "artifact_digest": row.get::<_,Option<String>>(10),
        "execution_result_digest": row.get::<_,Option<String>>(11),
        "decisions": decisions.iter().map(|d| json!({
            "decision": d.get::<_,String>(0),
            "reviewer_actor_id": d.get::<_,String>(1),
            "reviewer_person_id": d.get::<_,Option<String>>(2),
            "independence_kind": d.get::<_,String>(3),
            "reason": d.get::<_,String>(4),
            "team_independent_acceptance": d.get::<_,String>(3)=="team_independent" && d.get::<_,String>(0)=="approve",
        })).collect::<Vec<_>>(),
    }}))
}

pub(crate) async fn inspect_completion(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    work_id: &str,
) -> PgResult<Value> {
    let runtime = tx
        .query_opt(
            "SELECT state, selected_completion_id FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id=$3",
            &[&tenant, &project, &work_id],
        )
        .await?;
    let Some(rt) = runtime else {
        return Ok(json!({"completion": Value::Null, "runtime_state": Value::Null}));
    };
    let state: String = rt.get(0);
    let selected: Option<String> = rt.get(1);
    let receipt = if let Some(id) = selected.as_deref() {
        tx.query_opt(
            "SELECT id, contract_hash, policy, independence_kind, evidence_id, execution_id,
                    approved_by_person_id, submitted_by_person_id, approved_by_json
             FROM awr_team.completion_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &id],
        )
        .await?
    } else {
        None
    };
    Ok(json!({
        "runtime_state": state,
        "selected_completion_id": selected,
        "completion": receipt.map(|r| json!({
            "receipt_id": r.get::<_,String>(0),
            "contract_hash": r.get::<_,String>(1),
            "policy": r.get::<_,String>(2),
            "independence_kind": r.get::<_,Option<String>>(3),
            "evidence_id": r.get::<_,Option<String>>(4),
            "execution_id": r.get::<_,Option<String>>(5),
            "approved_by_person_id": r.get::<_,Option<String>>(6),
            "submitted_by_person_id": r.get::<_,Option<String>>(7),
            "approved_by": r.get::<_,Value>(8),
            "provider_private_session": Value::Null,
        })),
    }))
}
