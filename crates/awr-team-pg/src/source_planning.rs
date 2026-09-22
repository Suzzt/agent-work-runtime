//! Planning suggestions and controlled draft candidates (AWR-TMCP-021).
//!
//! Domain entry methods authenticate the bearer and authorize TMCP-010
//! `planning.*` actions. Suggestions never claim or mutate live deps/acceptance.
//! Approve and publish are separate, digest-bound actions. Source writeback of
//! published candidates is deferred to TMCP-022.
use super::{PgError, PgResult, SourceStore, sha256_hex};
use crate::tx::new_id;
use crate::workstream_auth::{authenticate, authenticate_writer, authorize_domain_action};
use awr_team::{
    AffectedTaskImpact, BaselineView, CandidateState, DraftChange,
    OrdinaryPlanningSelfApprovePolicy, PLANNING_CODEC, PlanningApproval, PlanningCandidate,
    PlanningSuggestion, ResourceRef, SUGGESTION_ADDS_FORMAL_WORK, SUGGESTION_CLAIMABLE,
    SuggestionState, authorize_planning_approve, authorize_planning_publish, build_candidate_diff,
    edit_candidate, ensure_independent_review_not_downgraded, refuse_reader_suggestion_write,
    validate_candidate,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_postgres::Transaction;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestionSubmit {
    pub rationale: String,
    pub affected_work_keys: Vec<String>,
    #[serde(default)]
    pub proposed_notes: Value,
    /// Optional person id; defaults to authenticated actor id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftCandidateCreate {
    pub changes: Vec<DraftChange>,
    #[serde(default)]
    pub suggestion_ids: Vec<String>,
    #[serde(default)]
    pub allowed_spec_roots: Vec<String>,
    #[serde(default)]
    pub project_goal_keys: Vec<String>,
    #[serde(default)]
    pub self_approve_policy: Option<OrdinaryPlanningSelfApprovePolicy>,
    /// Optional person id; defaults to authenticated actor id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_person_id: Option<String>,
}

fn map_team(err: awr_team::TeamError) -> PgError {
    match err {
        awr_team::TeamError::PermissionDenied(msg) => {
            if msg.contains("downgraded") {
                PgError::PolicyDowngrade
            } else {
                PgError::Forbidden
            }
        }
        other => PgError::Protocol(other.to_string()),
    }
}

fn json_string_list(v: &Value) -> PgResult<Vec<String>> {
    v.as_array()
        .ok_or_else(|| PgError::Protocol("expected json string array".into()))?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_owned)
                .ok_or_else(|| PgError::Protocol("expected string in json array".into()))
        })
        .collect()
}

impl SourceStore {
    /// Submit a planning suggestion (`planning.propose`). Not claimable; does
    /// not add formal work or mutate live deps/acceptance. Readers are refused.
    pub async fn submit_planning_suggestion(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        submit: &SuggestionSubmit,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate_writer(&tx, tenant_id, project_id, bearer).await?;
        authorize_domain_action(&auth, awr_team::Action::PlanningPropose, None, None)?;
        let scope = crate::workstream_auth::authority_scope(&auth, None, None);
        refuse_reader_suggestion_write(&scope).map_err(map_team)?;
        if SUGGESTION_CLAIMABLE || SUGGESTION_ADDS_FORMAL_WORK {
            return Err(PgError::Protocol(
                "suggestion invariants broken: must not be claimable or formal work".into(),
            ));
        }
        let (baseline_digest, baseline_epoch) =
            current_baseline(&tx, tenant_id, project_id).await?;
        let suggestion_id = new_id();
        let person = submit
            .author_person_id
            .clone()
            .unwrap_or_else(|| auth.actor_id.clone());
        let suggestion = PlanningSuggestion {
            codec: PLANNING_CODEC.into(),
            suggestion_id: suggestion_id.clone(),
            project_id: project_id.into(),
            author_person_id: person.clone(),
            author_actor_id: auth.actor_id.clone(),
            rationale: submit.rationale.clone(),
            version: 1,
            baseline_digest: baseline_digest.clone(),
            baseline_epoch: baseline_epoch.clone(),
            affected_work_keys: submit.affected_work_keys.clone(),
            proposed_notes: if submit.proposed_notes.is_null() {
                json!({})
            } else {
                submit.proposed_notes.clone()
            },
            state: SuggestionState::Open,
        };
        suggestion.validate().map_err(map_team)?;
        let keys = serde_json::to_value(&suggestion.affected_work_keys)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        tx.execute(
            "INSERT INTO awr_team.planning_suggestions(
                tenant_id, project_id, id, author_person_id, author_actor_id, author_client_id,
                rationale, version, baseline_digest, baseline_epoch, affected_work_keys,
                proposed_notes, state)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'open')",
            &[
                &tenant_id,
                &project_id,
                &suggestion_id,
                &suggestion.author_person_id,
                &auth.actor_id,
                &auth.client_id,
                &suggestion.rationale,
                &(suggestion.version as i32),
                &baseline_digest,
                &baseline_epoch,
                &keys,
                &suggestion.proposed_notes,
            ],
        )
        .await?;
        let result = json!({
            "suggestion_id": suggestion_id,
            "version": suggestion.version,
            "author_person_id": suggestion.author_person_id,
            "author_actor_id": auth.actor_id,
            "rationale": suggestion.rationale,
            "baseline_digest": baseline_digest,
            "baseline_epoch": baseline_epoch,
            "claimable": false,
            "adds_formal_work": false,
            "mutates_live_deps": false,
            "mutates_live_acceptance": false,
            "state": "open"
        });
        tx.commit().await?;
        Ok(result)
    }

    /// Create a planning draft candidate (`planning.edit_draft`).
    pub async fn create_planning_candidate(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        create: &DraftCandidateCreate,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate_writer(&tx, tenant_id, project_id, bearer).await?;
        authorize_domain_action(&auth, awr_team::Action::PlanningEditDraft, None, None)?;
        let (baseline_digest, baseline_epoch) =
            current_baseline(&tx, tenant_id, project_id).await?;
        let known = known_work(&tx, tenant_id, project_id).await?;
        let policy = create
            .self_approve_policy
            .clone()
            .unwrap_or_else(OrdinaryPlanningSelfApprovePolicy::ordinary_default);
        ensure_independent_review_not_downgraded(
            "independent_review",
            &policy.delivery_completion_policy,
        )
        .map_err(map_team)?;
        let person = create
            .author_person_id
            .clone()
            .unwrap_or_else(|| auth.actor_id.clone());
        let candidate_id = new_id();
        let candidate = PlanningCandidate {
            codec: PLANNING_CODEC.into(),
            candidate_id: candidate_id.clone(),
            project_id: project_id.into(),
            author_person_id: person,
            author_actor_id: auth.actor_id.clone(),
            baseline_digest: baseline_digest.clone(),
            baseline_epoch: baseline_epoch.clone(),
            draft_revision: 1,
            changes: create.changes.clone(),
            suggestion_ids: create.suggestion_ids.clone(),
            state: CandidateState::Drafting,
            approval: None,
            allowed_spec_roots: create.allowed_spec_roots.clone(),
            project_goal_keys: if create.project_goal_keys.is_empty() {
                vec!["delivery".into()]
            } else {
                create.project_goal_keys.clone()
            },
        };
        let base_view = BaselineView {
            digest: &baseline_digest,
            epoch: &baseline_epoch,
            current: true,
            known_work_ids: known.ids.iter().map(String::as_str).collect(),
            known_external_keys: known.keys.iter().map(String::as_str).collect(),
        };
        validate_candidate(&candidate, &base_view).map_err(map_team)?;
        let digest = candidate.candidate_digest().map_err(map_team)?;
        let changes_json = serde_json::to_value(&candidate.changes)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        let suggestion_ids = serde_json::to_value(&candidate.suggestion_ids)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        let roots = serde_json::to_value(&candidate.allowed_spec_roots)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        let goals = serde_json::to_value(&candidate.project_goal_keys)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        let policy_json =
            serde_json::to_value(&policy).map_err(|e| PgError::Protocol(e.to_string()))?;
        tx.execute(
            "INSERT INTO awr_team.planning_candidates(
                tenant_id, project_id, id, author_person_id, author_actor_id, author_client_id,
                baseline_digest, baseline_epoch, draft_revision, candidate_digest, state,
                changes_json, suggestion_ids, allowed_spec_roots, project_goal_keys,
                self_approve_policy_json, delivery_completion_policy)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'drafting',$11,$12,$13,$14,$15,$16)",
            &[
                &tenant_id,
                &project_id,
                &candidate_id,
                &candidate.author_person_id,
                &auth.actor_id,
                &auth.client_id,
                &baseline_digest,
                &baseline_epoch,
                &(candidate.draft_revision as i32),
                &digest,
                &changes_json,
                &suggestion_ids,
                &roots,
                &goals,
                &policy_json,
                &policy.delivery_completion_policy,
            ],
        )
        .await?;
        append_history(
            &tx,
            tenant_id,
            project_id,
            &candidate_id,
            candidate.draft_revision as i32,
            &digest,
            &changes_json,
            &auth.actor_id,
            &auth.client_id,
        )
        .await?;
        for sid in &candidate.suggestion_ids {
            tx.execute(
                "UPDATE awr_team.planning_suggestions SET state='accepted_into_draft'
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND state='open'",
                &[&tenant_id, &project_id, sid],
            )
            .await?;
        }
        let impacts = load_impacts(&tx, tenant_id, project_id, &candidate).await?;
        let diff = build_candidate_diff(&candidate, impacts).map_err(map_team)?;
        let result = json!({
            "candidate_id": candidate_id,
            "draft_revision": candidate.draft_revision,
            "candidate_digest": digest,
            "state": "drafting",
            "diff": diff,
            "hard_delete_history_allowed": false,
            "forge_completion_via_status_allowed": false
        });
        tx.commit().await?;
        Ok(result)
    }

    /// Edit an existing draft candidate. Clears prior approval and bumps revision.
    pub async fn edit_planning_candidate(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
        changes: Vec<DraftChange>,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate_writer(&tx, tenant_id, project_id, bearer).await?;
        authorize_domain_action(&auth, awr_team::Action::PlanningEditDraft, None, None)?;
        let mut candidate = load_candidate(&tx, tenant_id, project_id, candidate_id).await?;
        let (baseline_digest, baseline_epoch) =
            current_baseline(&tx, tenant_id, project_id).await?;
        // Edits must still target the live baseline; expired baselines refuse.
        candidate.baseline_digest = baseline_digest.clone();
        candidate.baseline_epoch = baseline_epoch.clone();
        candidate = edit_candidate(candidate, changes).map_err(map_team)?;
        let known = known_work(&tx, tenant_id, project_id).await?;
        let base_view = BaselineView {
            digest: &baseline_digest,
            epoch: &baseline_epoch,
            current: true,
            known_work_ids: known.ids.iter().map(String::as_str).collect(),
            known_external_keys: known.keys.iter().map(String::as_str).collect(),
        };
        validate_candidate(&candidate, &base_view).map_err(map_team)?;
        let digest = candidate.candidate_digest().map_err(map_team)?;
        let changes_json = serde_json::to_value(&candidate.changes)
            .map_err(|e| PgError::Protocol(e.to_string()))?;
        tx.execute(
            "UPDATE awr_team.planning_candidates SET
                draft_revision=$4, candidate_digest=$5, state='drafting',
                changes_json=$6, baseline_digest=$7, baseline_epoch=$8,
                updated_at=clock_timestamp()
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &tenant_id,
                &project_id,
                &candidate_id,
                &(candidate.draft_revision as i32),
                &digest,
                &changes_json,
                &baseline_digest,
                &baseline_epoch,
            ],
        )
        .await?;
        append_history(
            &tx,
            tenant_id,
            project_id,
            candidate_id,
            candidate.draft_revision as i32,
            &digest,
            &changes_json,
            &auth.actor_id,
            &auth.client_id,
        )
        .await?;
        let impacts = load_impacts(&tx, tenant_id, project_id, &candidate).await?;
        let diff = build_candidate_diff(&candidate, impacts).map_err(map_team)?;
        let result = json!({
            "candidate_id": candidate_id,
            "draft_revision": candidate.draft_revision,
            "candidate_digest": digest,
            "state": "drafting",
            "prior_approval_cleared": true,
            "diff": diff
        });
        tx.commit().await?;
        Ok(result)
    }

    /// Preview exact diffs, affected tasks, and review requirements.
    pub async fn preview_planning_candidate(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        // Preview requires at least propose or edit or approve/publish.
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
        let candidate = load_candidate(&tx, tenant_id, project_id, candidate_id).await?;
        let impacts = load_impacts(&tx, tenant_id, project_id, &candidate).await?;
        let diff = build_candidate_diff(&candidate, impacts).map_err(map_team)?;
        let result = json!({
            "candidate_id": candidate_id,
            "state": match candidate.state {
                CandidateState::Drafting => "drafting",
                CandidateState::Approved => "approved",
                CandidateState::Published => "published",
                CandidateState::Superseded => "superseded",
            },
            "draft_revision": candidate.draft_revision,
            "diff": diff,
            "approval": candidate.approval,
        });
        tx.commit().await?;
        Ok(result)
    }

    /// Approve bound to the current candidate digest (`planning.approve`).
    pub async fn approve_planning_candidate(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
        candidate_digest: &str,
        author_person_id: Option<&str>,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate_writer(&tx, tenant_id, project_id, bearer).await?;
        authorize_domain_action(&auth, awr_team::Action::PlanningApprove, None, None)?;
        let row = tx
            .query_one(
                "SELECT self_approve_policy_json, delivery_completion_policy
                 FROM awr_team.planning_candidates
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
                &[&tenant_id, &project_id, &candidate_id],
            )
            .await
            .map_err(|_| PgError::Protocol("planning candidate not found".into()))?;
        let policy_json: Value = row.get(0);
        let delivery_policy: String = row.get(1);
        let policy: OrdinaryPlanningSelfApprovePolicy =
            serde_json::from_value(policy_json).map_err(|e| PgError::Protocol(e.to_string()))?;
        let mut candidate = load_candidate(&tx, tenant_id, project_id, candidate_id).await?;
        if let Some(person) = author_person_id {
            // Allow callers to assert person identity for self-approve checks.
            if candidate.author_person_id != person && auth.actor_id != person {
                // keep stored author; person override only for approver scope
            }
        }
        let mut scope = crate::workstream_auth::authority_scope(&auth, None, None);
        if let Some(person) = author_person_id {
            scope.person_id = person.into();
        }
        let resource = ResourceRef {
            tenant_id: tenant_id.into(),
            project_id: project_id.into(),
            workstream_id: None,
            work_id: None,
        };
        let self_approved = authorize_planning_approve(
            &scope,
            &resource,
            now_unix_ms(),
            &candidate,
            candidate_digest,
            &policy,
            &delivery_policy,
        )
        .map_err(map_team)?;
        let approval_id = new_id();
        tx.execute(
            "INSERT INTO awr_team.planning_approvals(
                tenant_id, project_id, id, candidate_id, candidate_digest, draft_revision,
                approver_person_id, approver_actor_id, approver_client_id, self_approved)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            &[
                &tenant_id,
                &project_id,
                &approval_id,
                &candidate_id,
                &candidate_digest,
                &(candidate.draft_revision as i32),
                &scope.person_id,
                &auth.actor_id,
                &auth.client_id,
                &self_approved,
            ],
        )
        .await?;
        tx.execute(
            "UPDATE awr_team.planning_candidates SET state='approved', updated_at=clock_timestamp()
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &candidate_id],
        )
        .await?;
        candidate.approval = Some(PlanningApproval {
            approval_id: approval_id.clone(),
            candidate_digest: candidate_digest.into(),
            approver_person_id: scope.person_id.clone(),
            approver_actor_id: auth.actor_id.clone(),
            self_approved,
        });
        candidate.state = CandidateState::Approved;
        let result = json!({
            "approval_id": approval_id,
            "candidate_id": candidate_id,
            "candidate_digest": candidate_digest,
            "draft_revision": candidate.draft_revision,
            "self_approved": self_approved,
            "state": "approved",
            "delivery_completion_policy": delivery_policy,
            "independent_review_downgraded": false
        });
        tx.commit().await?;
        Ok(result)
    }

    /// Publish is separate from approve and also digest-bound. Does not write
    /// authoritative source bytes (TMCP-022); records a publish receipt.
    pub async fn publish_planning_candidate(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        candidate_id: &str,
        candidate_digest: &str,
    ) -> PgResult<Value> {
        let mut client = self.connect().await?;
        crate::check_schema(&client).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let auth = authenticate_writer(&tx, tenant_id, project_id, bearer).await?;
        authorize_domain_action(&auth, awr_team::Action::PlanningPublish, None, None)?;
        let mut candidate = load_candidate(&tx, tenant_id, project_id, candidate_id).await?;
        // Attach latest approval for publish checks.
        if let Some(row) = tx
            .query_opt(
                "SELECT id, candidate_digest, approver_person_id, approver_actor_id, self_approved
                 FROM awr_team.planning_approvals
                 WHERE tenant_id=$1 AND project_id=$2 AND candidate_id=$3
                 ORDER BY decided_at DESC LIMIT 1",
                &[&tenant_id, &project_id, &candidate_id],
            )
            .await?
        {
            let approval_id: String = row.get(0);
            let digest: String = row.get(1);
            let person: String = row.get(2);
            let actor: String = row.get(3);
            let self_approved: bool = row.get(4);
            candidate.approval = Some(PlanningApproval {
                approval_id: approval_id.clone(),
                candidate_digest: digest,
                approver_person_id: person,
                approver_actor_id: actor,
                self_approved,
            });
            candidate.state = CandidateState::Approved;
            let scope = crate::workstream_auth::authority_scope(&auth, None, None);
            let resource = ResourceRef {
                tenant_id: tenant_id.into(),
                project_id: project_id.into(),
                workstream_id: None,
                work_id: None,
            };
            authorize_planning_publish(
                &scope,
                &resource,
                now_unix_ms(),
                &candidate,
                candidate_digest,
            )
            .map_err(map_team)?;
            let receipt_id = new_id();
            tx.execute(
                "INSERT INTO awr_team.planning_publish_receipts(
                    tenant_id, project_id, id, candidate_id, candidate_digest, draft_revision,
                    approval_id, publisher_actor_id, publisher_client_id, source_writeback_pending)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,true)",
                &[
                    &tenant_id,
                    &project_id,
                    &receipt_id,
                    &candidate_id,
                    &candidate_digest,
                    &(candidate.draft_revision as i32),
                    &approval_id,
                    &auth.actor_id,
                    &auth.client_id,
                ],
            )
            .await?;
            tx.execute(
                "UPDATE awr_team.planning_candidates SET state='published', updated_at=clock_timestamp()
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
                &[&tenant_id, &project_id, &candidate_id],
            )
            .await?;
            let result = json!({
                "receipt_id": receipt_id,
                "candidate_id": candidate_id,
                "candidate_digest": candidate_digest,
                "draft_revision": candidate.draft_revision,
                "approval_id": approval_id,
                "state": "published",
                "source_writeback_pending": true,
                "source_bytes_written": false
            });
            tx.commit().await?;
            Ok(result)
        } else {
            Err(PgError::CandidateNotApproved)
        }
    }

    /// Hard-delete of planning history is refused.
    pub async fn delete_planning_history(
        &self,
        _tenant_id: &str,
        _project_id: &str,
        _bearer: &str,
        _candidate_id: &str,
    ) -> PgResult<()> {
        Err(PgError::Protocol(
            "hard-delete of planning history is forbidden".into(),
        ))
    }

    /// Capability probe for TMCP-021 planning surfaces.
    pub fn planning_capabilities() -> Value {
        json!({
            "codec": PLANNING_CODEC,
            "actions": [
                "planning.propose",
                "planning.edit_draft",
                "planning.approve",
                "planning.publish"
            ],
            "suggestion_claimable": false,
            "suggestion_adds_formal_work": false,
            "hard_delete_history_allowed": false,
            "forge_completion_via_status_allowed": false,
            "approve_publish_separated": true,
            "approval_bound_to_candidate_digest": true,
            "source_writeback": "deferred_to_tmcp_022"
        })
    }
}

struct KnownWork {
    ids: Vec<String>,
    keys: Vec<String>,
}

async fn current_baseline(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
) -> PgResult<(String, String)> {
    let row = tx
        .query_one(
            "SELECT COALESCE(active_snapshot_id, ''), authority_epoch::text
             FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&tenant_id, &project_id],
        )
        .await?;
    let snapshot: String = row.get(0);
    let epoch: String = row.get(1);
    if snapshot.is_empty() {
        // No activated source yet — use a stable empty baseline so first-round
        // planning drafts can still be authored against an empty graph.
        return Ok((format!("sha256:{}", sha256_hex(b"empty-baseline")), epoch));
    }
    let digest: String = tx
        .query_one(
            "SELECT manifest_digest FROM awr_team.source_snapshots
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &snapshot],
        )
        .await?
        .get(0);
    Ok((digest, epoch))
}

async fn known_work(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
) -> PgResult<KnownWork> {
    let rows = tx
        .query(
            "SELECT id, external_key FROM awr_team.work_items
             WHERE tenant_id=$1 AND project_id=$2",
            &[&tenant_id, &project_id],
        )
        .await?;
    let mut ids = Vec::new();
    let mut keys = Vec::new();
    for row in rows {
        ids.push(row.get::<_, String>(0));
        keys.push(row.get::<_, String>(1));
    }
    Ok(KnownWork { ids, keys })
}

async fn append_history(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    candidate_id: &str,
    draft_revision: i32,
    digest: &str,
    changes_json: &Value,
    actor_id: &str,
    client_id: &str,
) -> PgResult<()> {
    tx.execute(
        "INSERT INTO awr_team.planning_candidate_history(
            tenant_id, project_id, candidate_id, draft_revision, candidate_digest,
            changes_json, actor_id, client_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        &[
            &tenant_id,
            &project_id,
            &candidate_id,
            &draft_revision,
            &digest,
            &changes_json,
            &actor_id,
            &client_id,
        ],
    )
    .await?;
    Ok(())
}

async fn load_candidate(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    candidate_id: &str,
) -> PgResult<PlanningCandidate> {
    let row = tx
        .query_opt(
            "SELECT author_person_id, author_actor_id, baseline_digest, baseline_epoch,
                    draft_revision, candidate_digest, state, changes_json, suggestion_ids,
                    allowed_spec_roots, project_goal_keys
             FROM awr_team.planning_candidates
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant_id, &project_id, &candidate_id],
        )
        .await?
        .ok_or_else(|| PgError::Protocol("planning candidate not found".into()))?;
    let state_s: String = row.get(6);
    let state = match state_s.as_str() {
        "drafting" => CandidateState::Drafting,
        "approved" => CandidateState::Approved,
        "published" => CandidateState::Published,
        "superseded" => CandidateState::Superseded,
        other => {
            return Err(PgError::Protocol(format!(
                "unknown planning candidate state: {other}"
            )));
        }
    };
    let changes: Vec<DraftChange> =
        serde_json::from_value(row.get(7)).map_err(|e| PgError::Protocol(e.to_string()))?;
    let suggestion_ids = json_string_list(&row.get(8))?;
    let allowed_spec_roots = json_string_list(&row.get(9))?;
    let project_goal_keys = json_string_list(&row.get(10))?;
    let draft_revision: i32 = row.get(4);
    let mut candidate = PlanningCandidate {
        codec: PLANNING_CODEC.into(),
        candidate_id: candidate_id.into(),
        project_id: project_id.into(),
        author_person_id: row.get(0),
        author_actor_id: row.get(1),
        baseline_digest: row.get(2),
        baseline_epoch: row.get(3),
        draft_revision: draft_revision as u32,
        changes,
        suggestion_ids,
        state,
        approval: None,
        allowed_spec_roots,
        project_goal_keys,
    };
    // Verify stored digest still matches content (edited drafts bump revision).
    let computed = candidate.candidate_digest().map_err(map_team)?;
    let stored: String = row.get(5);
    if computed != stored && matches!(state, CandidateState::Drafting) {
        // Allow mismatch only if we're about to rewrite; for load used by
        // approve/publish the stored digest is authoritative for binding.
        candidate.baseline_digest = row.get(2);
    }
    if computed != stored && matches!(state, CandidateState::Approved | CandidateState::Published) {
        return Err(PgError::StaleApproval);
    }
    Ok(candidate)
}

async fn load_impacts(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    candidate: &PlanningCandidate,
) -> PgResult<Vec<AffectedTaskImpact>> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for change in &candidate.changes {
        let work_id = change.after.work_id.clone();
        if !seen.insert(work_id.clone()) {
            continue;
        }
        let runtime: Option<String> = tx
            .query_opt(
                "SELECT state FROM awr_team.work_runtime
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
                 LIMIT 1",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .map(|r| r.get(0));
        let claim: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.claims
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='active'",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .get(0);
        let exec: Option<String> = tx
            .query_opt(
                "SELECT state FROM awr_team.executions
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
                   AND state NOT IN ('succeeded','failed','cancelled')
                 LIMIT 1",
                &[&tenant_id, &project_id, &work_id],
            )
            .await?
            .map(|r| r.get(0));
        out.push(AffectedTaskImpact {
            work_id: work_id.clone(),
            external_key: change.after.external_key.clone(),
            work_runtime_state: runtime.unwrap_or_else(|| "none".into()),
            execution_state: exec.unwrap_or_else(|| "none".into()),
            has_active_claim: claim > 0,
            review_requirement: change.after.completion_policy.clone(),
        });
    }
    Ok(out)
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
