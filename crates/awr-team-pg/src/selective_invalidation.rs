//! Team PG selective invalidation, boundary revalidation, planning changes (WS-032).
//!
//! Pure decision helpers mirror `awr-runtime::selective_invalidation` so Team PG
//! stays free of the SQLite runtime dependency. Persistence uses FORCE RLS and
//! the project coordination lock for revoke-race safety.
use crate::error::{PgError, PgResult};
use crate::tx::{bind_workstream_scope, new_id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio_postgres::Transaction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderChangeKind {
    NewVersionOrProgress,
    CurrentSelectionChanged,
    DeliveryRevoked,
    PinnedArtifactUnavailable,
}

impl ProviderChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewVersionOrProgress => "new_version_or_progress",
            Self::CurrentSelectionChanged => "current_selection_changed",
            Self::DeliveryRevoked => "delivery_revoked",
            Self::PinnedArtifactUnavailable => "pinned_artifact_unavailable",
        }
    }
    #[allow(dead_code)]
    fn parse(s: &str) -> PgResult<Self> {
        match s {
            "new_version_or_progress" => Ok(Self::NewVersionOrProgress),
            "current_selection_changed" => Ok(Self::CurrentSelectionChanged),
            "delivery_revoked" => Ok(Self::DeliveryRevoked),
            "pinned_artifact_unavailable" => Ok(Self::PinnedArtifactUnavailable),
            _ => Err(PgError::Protocol("unknown provider change kind".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptedConsumerEdge {
    pub dependency_id: String,
    pub consumer_work_id: String,
    pub provider_work_id: String,
    /// `fixed_delivery` or `current_contract`
    pub policy: String,
    pub credential_status: String,
    pub assessment_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectiveInvalidationPlan {
    pub provider_work_id: String,
    pub change: ProviderChangeKind,
    pub reevaluate: Vec<String>,
    pub leave_valid: Vec<String>,
    pub unaffected_work_ids: Vec<String>,
}

pub fn select_downstream_reevaluation(
    provider_work_id: &str,
    change: ProviderChangeKind,
    consumers: &[AdoptedConsumerEdge],
    all_project_work_ids: &[String],
) -> SelectiveInvalidationPlan {
    let mut reevaluate = BTreeSet::new();
    let mut leave_valid = BTreeSet::new();
    let mut consumer_ids = BTreeSet::new();
    for edge in consumers {
        if edge.provider_work_id != provider_work_id {
            continue;
        }
        consumer_ids.insert(edge.consumer_work_id.clone());
        if edge.credential_status == "revoked" || edge.assessment_status == "revoked" {
            reevaluate.insert(edge.consumer_work_id.clone());
            continue;
        }
        let fixed = edge.policy == "fixed_delivery";
        let needs = match (fixed, change) {
            (true, ProviderChangeKind::NewVersionOrProgress) => false,
            (true, ProviderChangeKind::CurrentSelectionChanged) => false,
            (
                true,
                ProviderChangeKind::DeliveryRevoked | ProviderChangeKind::PinnedArtifactUnavailable,
            ) => true,
            (false, _) => true,
        };
        if needs {
            reevaluate.insert(edge.consumer_work_id.clone());
        } else {
            leave_valid.insert(edge.consumer_work_id.clone());
        }
    }
    let unaffected: Vec<String> = all_project_work_ids
        .iter()
        .filter(|w| w.as_str() != provider_work_id && !consumer_ids.contains(w.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    SelectiveInvalidationPlan {
        provider_work_id: provider_work_id.into(),
        change,
        reevaluate: reevaluate.into_iter().collect(),
        leave_valid: leave_valid.into_iter().collect(),
        unaffected_work_ids: unaffected,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBoundary {
    Prepare,
    Dispatch,
    Complete,
}

impl ExecutionBoundary {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Dispatch => "dispatch",
            Self::Complete => "complete",
        }
    }
    #[allow(dead_code)]
    fn parse(s: &str) -> PgResult<Self> {
        match s {
            "prepare" => Ok(Self::Prepare),
            "dispatch" => Ok(Self::Dispatch),
            "complete" => Ok(Self::Complete),
            _ => Err(PgError::Protocol("unknown execution boundary".into())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryDecision {
    Allow,
    BlockNewEffects,
    KeepEffectsAssignRecovery,
}

impl BoundaryDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::BlockNewEffects => "block_new_effects",
            Self::KeepEffectsAssignRecovery => "keep_effects_assign_recovery",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundarySnapshot {
    pub boundary: ExecutionBoundary,
    pub work_id: String,
    pub execution_id: Option<String>,
    pub dependencies_satisfied: bool,
    pub revoke_or_invalidation_observed: bool,
    pub has_real_effects: bool,
    pub unrelated_work_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryRevalidation {
    pub decision: BoundaryDecision,
    pub recovery_duty: bool,
    pub erase_effects: bool,
    pub unrelated_may_continue: bool,
    pub reason: String,
}

pub fn revalidate_execution_boundary(snapshot: &BoundarySnapshot) -> BoundaryRevalidation {
    if snapshot.dependencies_satisfied && !snapshot.revoke_or_invalidation_observed {
        return BoundaryRevalidation {
            decision: BoundaryDecision::Allow,
            recovery_duty: false,
            erase_effects: false,
            unrelated_may_continue: true,
            reason: "dependencies satisfied at boundary".into(),
        };
    }
    if snapshot.has_real_effects {
        return BoundaryRevalidation {
            decision: BoundaryDecision::KeepEffectsAssignRecovery,
            recovery_duty: true,
            erase_effects: false,
            unrelated_may_continue: true,
            reason: "mid-execution invalidation retains effects and recovery duty".into(),
        };
    }
    let reason = match snapshot.boundary {
        ExecutionBoundary::Prepare => "prepare revalidation blocked revoke/stale race",
        ExecutionBoundary::Dispatch => "dispatch revalidation blocked revoke/stale race",
        ExecutionBoundary::Complete => "complete revalidation blocked revoke/stale race",
    };
    BoundaryRevalidation {
        decision: BoundaryDecision::BlockNewEffects,
        recovery_duty: false,
        erase_effects: false,
        unrelated_may_continue: true,
        reason: reason.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelSplitRelation {
    pub kind: String,
    pub from_work_id: String,
    pub to_work_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedPlanningChange {
    pub change_id: String,
    pub discovered_by: String,
    pub old_graph_version: String,
    pub new_graph_version: String,
    pub old_acceptance_contract: String,
    pub new_acceptance_contract: String,
    pub affected_work_ids: Vec<String>,
    pub cancel_split_relations: Vec<CancelSplitRelation>,
    pub continue_conditions: Vec<String>,
    pub status: String,
    pub confirmed_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectiveInvalidateRequest {
    pub request_key: String,
    pub event_id: String,
    pub provider_work_id: String,
    pub change: ProviderChangeKind,
    pub consumers: Vec<AdoptedConsumerEdge>,
    pub all_project_work_ids: Vec<String>,
    pub now_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryRevalidateRequest {
    pub request_key: String,
    pub check_id: String,
    pub snapshot: BoundarySnapshot,
    pub now_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordPlanningChangeRequest {
    pub request_key: String,
    pub change_id: String,
    pub discovered_by: String,
    pub old_graph_version: String,
    pub new_graph_version: String,
    pub old_acceptance_contract: String,
    pub new_acceptance_contract: String,
    pub affected_work_ids: Vec<String>,
    pub cancel_split_relations: Vec<CancelSplitRelation>,
    pub continue_conditions: Vec<String>,
    pub all_project_work_ids: Vec<String>,
    pub now_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecidePlanningChangeRequest {
    pub request_key: String,
    pub change_id: String,
    pub actor_id: String,
    pub now_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectiveInvalidationReceipt {
    pub request_key: String,
    pub subject_id: String,
    pub op: String,
    pub event_id: String,
    pub replayed: bool,
    pub created_at_ms: i64,
}

async fn lock_project(tx: &Transaction<'_>, tenant: &str, project: &str) -> PgResult<()> {
    tx.query_one(
        "SELECT id FROM awr_team.projects WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
        &[&tenant, &project],
    )
    .await?;
    Ok(())
}

pub struct SelectiveInvalidationStore {
    pool: crate::PgPool,
}

impl SelectiveInvalidationStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            pool: crate::PgPool::new(url),
        }
    }
    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self {
            pool: crate::PgPool::from_config(config),
        }
    }
    async fn connect(&self) -> PgResult<crate::PgClient> {
        self.pool.get().await
    }

    /// Selectively invalidate downstream bindings for affected consumers only.
    /// Fixed-delivery leave_valid consumers keep `dependency_bindings.valid=true`.
    pub async fn apply_selective_invalidation(
        &self,
        tenant: &str,
        project: &str,
        req: &SelectiveInvalidateRequest,
    ) -> PgResult<(SelectiveInvalidationPlan, SelectiveInvalidationReceipt)> {
        if req.now_ms < 0 || req.request_key.is_empty() || req.event_id.is_empty() {
            return Err(PgError::Protocol(
                "invalid selective invalidate request".into(),
            ));
        }
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        if let Some(receipt) = load_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "selective_invalidate",
        )
        .await?
        {
            let plan = load_plan(&tx, tenant, project, &receipt.subject_id).await?;
            tx.commit().await?;
            return Ok((
                plan,
                SelectiveInvalidationReceipt {
                    request_key: receipt.request_key,
                    subject_id: receipt.subject_id,
                    op: receipt.op,
                    event_id: receipt.event_id,
                    replayed: true,
                    created_at_ms: receipt.created_at_ms,
                },
            ));
        }
        lock_project(&tx, tenant, project).await?;
        let plan = select_downstream_reevaluation(
            &req.provider_work_id,
            req.change,
            &req.consumers,
            &req.all_project_work_ids,
        );
        // Invalidate only reevaluate consumers' bindings for this provider.
        for consumer in &plan.reevaluate {
            tx.execute(
                "UPDATE awr_team.dependency_bindings SET valid=false
                 WHERE tenant_id=$1 AND project_id=$2
                   AND downstream_work_id=$3 AND upstream_work_id=$4 AND valid=true",
                &[&tenant, &project, consumer, &req.provider_work_id],
            )
            .await?;
        }
        let body = json!({
            "provider_work_id": plan.provider_work_id,
            "change": plan.change.as_str(),
            "reevaluate": plan.reevaluate,
            "leave_valid": plan.leave_valid,
            "unaffected_work_ids": plan.unaffected_work_ids,
        });
        tx.execute(
            "INSERT INTO awr_team.selective_invalidation_events(
                tenant_id, project_id, id, provider_work_id, change_kind,
                reevaluate_json, leave_valid_json, unaffected_json, created_at_ms, body_json)
             VALUES ($1,$2,$3,$4,$5,$6::jsonb,$7::jsonb,$8::jsonb,$9,$10::jsonb)",
            &[
                &tenant,
                &project,
                &req.event_id,
                &req.provider_work_id,
                &req.change.as_str(),
                &json!(plan.reevaluate),
                &json!(plan.leave_valid),
                &json!(plan.unaffected_work_ids),
                &req.now_ms,
                &body,
            ],
        )
        .await?;
        store_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            &req.event_id,
            "selective_invalidate",
            &req.event_id,
            req.now_ms,
        )
        .await?;
        tx.commit().await?;
        Ok((
            plan,
            SelectiveInvalidationReceipt {
                request_key: req.request_key.clone(),
                subject_id: req.event_id.clone(),
                op: "selective_invalidate".into(),
                event_id: req.event_id.clone(),
                replayed: false,
                created_at_ms: req.now_ms,
            },
        ))
    }

    /// Atomic prepare/dispatch/complete revalidation under the project lock.
    pub async fn revalidate_boundary(
        &self,
        tenant: &str,
        project: &str,
        req: &BoundaryRevalidateRequest,
    ) -> PgResult<(BoundaryRevalidation, SelectiveInvalidationReceipt)> {
        if req.now_ms < 0 || req.request_key.is_empty() || req.check_id.is_empty() {
            return Err(PgError::Protocol(
                "invalid boundary revalidate request".into(),
            ));
        }
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        if let Some(receipt) = load_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "boundary_revalidate",
        )
        .await?
        {
            let decision = load_boundary(&tx, tenant, project, &receipt.subject_id).await?;
            tx.commit().await?;
            return Ok((
                decision,
                SelectiveInvalidationReceipt {
                    request_key: receipt.request_key,
                    subject_id: receipt.subject_id,
                    op: receipt.op,
                    event_id: receipt.event_id,
                    replayed: true,
                    created_at_ms: receipt.created_at_ms,
                },
            ));
        }
        lock_project(&tx, tenant, project).await?;
        // Re-check open planning blocks and invalid bindings under lock (revoke race).
        let planning_block: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.planning_change_action_blocks
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND active=true",
                &[&tenant, &project, &req.snapshot.work_id],
            )
            .await?
            .get(0);
        let invalid_bindings: i64 = tx
            .query_one(
                "SELECT count(*) FROM awr_team.dependency_bindings
                 WHERE tenant_id=$1 AND project_id=$2 AND downstream_work_id=$3 AND valid=false",
                &[&tenant, &project, &req.snapshot.work_id],
            )
            .await?
            .get(0);
        let mut snapshot = req.snapshot.clone();
        if planning_block > 0 || invalid_bindings > 0 {
            snapshot.revoke_or_invalidation_observed = true;
            snapshot.dependencies_satisfied = false;
        }
        let decision = revalidate_execution_boundary(&snapshot);
        let body = json!({
            "work_id": snapshot.work_id,
            "execution_id": snapshot.execution_id,
            "boundary": snapshot.boundary.as_str(),
            "decision": decision.decision.as_str(),
            "recovery_duty": decision.recovery_duty,
            "erase_effects": decision.erase_effects,
            "unrelated_may_continue": decision.unrelated_may_continue,
            "reason": decision.reason,
            "unrelated_work_ids": snapshot.unrelated_work_ids,
        });
        tx.execute(
            "INSERT INTO awr_team.execution_boundary_checks(
                tenant_id, project_id, id, work_id, execution_id, boundary, decision,
                recovery_duty, erase_effects, unrelated_may_continue, reason,
                created_at_ms, body_json)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13::jsonb)",
            &[
                &tenant,
                &project,
                &req.check_id,
                &snapshot.work_id,
                &snapshot.execution_id,
                &snapshot.boundary.as_str(),
                &decision.decision.as_str(),
                &decision.recovery_duty,
                &decision.erase_effects,
                &decision.unrelated_may_continue,
                &decision.reason,
                &req.now_ms,
                &body,
            ],
        )
        .await?;
        store_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            &req.check_id,
            "boundary_revalidate",
            &req.check_id,
            req.now_ms,
        )
        .await?;
        tx.commit().await?;
        Ok((
            decision,
            SelectiveInvalidationReceipt {
                request_key: req.request_key.clone(),
                subject_id: req.check_id.clone(),
                op: "boundary_revalidate".into(),
                event_id: req.check_id.clone(),
                replayed: false,
                created_at_ms: req.now_ms,
            },
        ))
    }

    /// Persist a scoped planning change and block affected actions first.
    pub async fn record_planning_change(
        &self,
        tenant: &str,
        project: &str,
        req: &RecordPlanningChangeRequest,
    ) -> PgResult<(
        ScopedPlanningChange,
        Vec<String>,
        SelectiveInvalidationReceipt,
    )> {
        if req.now_ms < 0
            || req.request_key.is_empty()
            || req.change_id.is_empty()
            || req.affected_work_ids.is_empty()
        {
            return Err(PgError::Protocol("invalid planning change request".into()));
        }
        if req.old_graph_version == req.new_graph_version
            && req.old_acceptance_contract == req.new_acceptance_contract
        {
            return Err(PgError::Protocol(
                "planning change must alter graph or acceptance contract".into(),
            ));
        }
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        if let Some(receipt) = load_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            "record_planning_change",
        )
        .await?
        {
            let change = load_planning_change(&tx, tenant, project, &receipt.subject_id).await?;
            let unrelated = unrelated_of(&change.affected_work_ids, &req.all_project_work_ids);
            tx.commit().await?;
            return Ok((
                change,
                unrelated,
                SelectiveInvalidationReceipt {
                    request_key: receipt.request_key,
                    subject_id: receipt.subject_id,
                    op: receipt.op,
                    event_id: receipt.event_id,
                    replayed: true,
                    created_at_ms: receipt.created_at_ms,
                },
            ));
        }
        lock_project(&tx, tenant, project).await?;
        let affected: Vec<String> = req
            .affected_work_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let unrelated = unrelated_of(&affected, &req.all_project_work_ids);
        let change = ScopedPlanningChange {
            change_id: req.change_id.clone(),
            discovered_by: req.discovered_by.clone(),
            old_graph_version: req.old_graph_version.clone(),
            new_graph_version: req.new_graph_version.clone(),
            old_acceptance_contract: req.old_acceptance_contract.clone(),
            new_acceptance_contract: req.new_acceptance_contract.clone(),
            affected_work_ids: affected.clone(),
            cancel_split_relations: req.cancel_split_relations.clone(),
            continue_conditions: req.continue_conditions.clone(),
            status: "affected_blocked".into(),
            confirmed_by: None,
        };
        let body = serde_json::to_value(&change)
            .map_err(|e| PgError::Protocol(format!("planning change encode: {e}")))?;
        tx.execute(
            "INSERT INTO awr_team.scoped_planning_changes(
                tenant_id, project_id, id, discovered_by,
                old_graph_version, new_graph_version,
                old_acceptance_contract, new_acceptance_contract,
                affected_json, cancel_split_json, continue_conditions_json,
                status, confirmed_by, created_at_ms, body_json)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9::jsonb,$10::jsonb,$11::jsonb,
                     'affected_blocked',NULL,$12,$13::jsonb)",
            &[
                &tenant,
                &project,
                &req.change_id,
                &req.discovered_by,
                &req.old_graph_version,
                &req.new_graph_version,
                &req.old_acceptance_contract,
                &req.new_acceptance_contract,
                &json!(affected),
                &json!(req.cancel_split_relations),
                &json!(req.continue_conditions),
                &req.now_ms,
                &body,
            ],
        )
        .await?;
        for work_id in &affected {
            tx.execute(
                "INSERT INTO awr_team.planning_change_action_blocks(
                    tenant_id, project_id, change_id, work_id, active)
                 VALUES ($1,$2,$3,$4,true)
                 ON CONFLICT (tenant_id, project_id, change_id, work_id)
                 DO UPDATE SET active=true",
                &[&tenant, &project, &req.change_id, work_id],
            )
            .await?;
        }
        store_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            &req.change_id,
            "record_planning_change",
            &req.change_id,
            req.now_ms,
        )
        .await?;
        tx.commit().await?;
        Ok((
            change,
            unrelated,
            SelectiveInvalidationReceipt {
                request_key: req.request_key.clone(),
                subject_id: req.change_id.clone(),
                op: "record_planning_change".into(),
                event_id: req.change_id.clone(),
                replayed: false,
                created_at_ms: req.now_ms,
            },
        ))
    }

    pub async fn confirm_planning_change(
        &self,
        tenant: &str,
        project: &str,
        req: &DecidePlanningChangeRequest,
    ) -> PgResult<(ScopedPlanningChange, SelectiveInvalidationReceipt)> {
        self.decide_planning_change(tenant, project, req, true)
            .await
    }

    pub async fn reject_planning_change(
        &self,
        tenant: &str,
        project: &str,
        req: &DecidePlanningChangeRequest,
    ) -> PgResult<(ScopedPlanningChange, SelectiveInvalidationReceipt)> {
        self.decide_planning_change(tenant, project, req, false)
            .await
    }

    async fn decide_planning_change(
        &self,
        tenant: &str,
        project: &str,
        req: &DecidePlanningChangeRequest,
        confirm: bool,
    ) -> PgResult<(ScopedPlanningChange, SelectiveInvalidationReceipt)> {
        if req.now_ms < 0 || req.request_key.is_empty() || req.actor_id.is_empty() {
            return Err(PgError::Protocol(
                "invalid planning decision request".into(),
            ));
        }
        let op = if confirm {
            "confirm_planning_change"
        } else {
            "reject_planning_change"
        };
        let status = if confirm { "confirmed" } else { "rejected" };
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        if let Some(receipt) = load_receipt(&tx, tenant, project, &req.request_key, op).await? {
            let change = load_planning_change(&tx, tenant, project, &receipt.subject_id).await?;
            tx.commit().await?;
            return Ok((
                change,
                SelectiveInvalidationReceipt {
                    request_key: receipt.request_key,
                    subject_id: receipt.subject_id,
                    op: receipt.op,
                    event_id: receipt.event_id,
                    replayed: true,
                    created_at_ms: receipt.created_at_ms,
                },
            ));
        }
        lock_project(&tx, tenant, project).await?;
        let row = tx
            .query_opt(
                "SELECT status FROM awr_team.scoped_planning_changes
                 WHERE tenant_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE",
                &[&tenant, &project, &req.change_id],
            )
            .await?
            .ok_or_else(|| PgError::Protocol("planning change missing".into()))?;
        let current: String = row.get(0);
        if current != "affected_blocked" {
            return Err(PgError::Protocol(
                "only affected_blocked planning changes can be decided".into(),
            ));
        }
        tx.execute(
            "UPDATE awr_team.scoped_planning_changes
             SET status=$4, confirmed_by=$5, decided_at_ms=$6,
                 body_json = body_json || jsonb_build_object(
                     'status', $4::text, 'confirmed_by', $5::text)
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &tenant,
                &project,
                &req.change_id,
                &status,
                &req.actor_id,
                &req.now_ms,
            ],
        )
        .await?;
        // Clear action blocks either way: confirmed unblocks under new graph;
        // rejected restores prior graph so affected works may continue under it
        // only after explicit operator recovery — still clear the *pending*
        // planning block marker so admission uses binding/adoption truth.
        tx.execute(
            "UPDATE awr_team.planning_change_action_blocks SET active=false
             WHERE tenant_id=$1 AND project_id=$2 AND change_id=$3",
            &[&tenant, &project, &req.change_id],
        )
        .await?;
        store_receipt(
            &tx,
            tenant,
            project,
            &req.request_key,
            &req.change_id,
            op,
            &req.change_id,
            req.now_ms,
        )
        .await?;
        let change = load_planning_change(&tx, tenant, project, &req.change_id).await?;
        tx.commit().await?;
        Ok((
            change,
            SelectiveInvalidationReceipt {
                request_key: req.request_key.clone(),
                subject_id: req.change_id.clone(),
                op: op.into(),
                event_id: req.change_id.clone(),
                replayed: false,
                created_at_ms: req.now_ms,
            },
        ))
    }

    pub async fn action_blocked(
        &self,
        tenant: &str,
        project: &str,
        work_id: &str,
    ) -> PgResult<Option<String>> {
        let mut client = self.connect().await?;
        let tx = client.transaction().await?;
        bind_workstream_scope(&tx, tenant, project).await?;
        let row = tx
            .query_opt(
                "SELECT change_id FROM awr_team.planning_change_action_blocks
                 WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND active=true
                 ORDER BY change_id LIMIT 1",
                &[&tenant, &project, &work_id],
            )
            .await?;
        tx.commit().await?;
        Ok(row.map(|r| r.get(0)))
    }
}

fn unrelated_of(affected: &[String], all: &[String]) -> Vec<String> {
    let set: BTreeSet<_> = affected.iter().cloned().collect();
    all.iter()
        .filter(|w| !set.contains(w.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

struct ReceiptRow {
    request_key: String,
    subject_id: String,
    op: String,
    event_id: String,
    created_at_ms: i64,
}

async fn load_receipt(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request_key: &str,
    op: &str,
) -> PgResult<Option<ReceiptRow>> {
    let row = tx
        .query_opt(
            "SELECT request_key, subject_id, op, event_id, created_at_ms
             FROM awr_team.selective_invalidation_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND request_key=$3",
            &[&tenant, &project, &request_key],
        )
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored_op: String = row.get(2);
    if stored_op != op {
        return Err(PgError::IdempotencyConflict);
    }
    Ok(Some(ReceiptRow {
        request_key: row.get(0),
        subject_id: row.get(1),
        op: stored_op,
        event_id: row.get(3),
        created_at_ms: row.get(4),
    }))
}

async fn store_receipt(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    request_key: &str,
    subject_id: &str,
    op: &str,
    event_id: &str,
    now_ms: i64,
) -> PgResult<()> {
    tx.execute(
        "INSERT INTO awr_team.selective_invalidation_receipts(
            tenant_id, project_id, request_key, subject_id, op, event_id, replayed, created_at_ms)
         VALUES ($1,$2,$3,$4,$5,$6,false,$7)",
        &[
            &tenant,
            &project,
            &request_key,
            &subject_id,
            &op,
            &event_id,
            &now_ms,
        ],
    )
    .await?;
    let _ = new_id();
    Ok(())
}

async fn load_plan(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    event_id: &str,
) -> PgResult<SelectiveInvalidationPlan> {
    let row = tx
        .query_one(
            "SELECT provider_work_id, change_kind, reevaluate_json, leave_valid_json, unaffected_json
             FROM awr_team.selective_invalidation_events
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &event_id],
        )
        .await?;
    let change = ProviderChangeKind::parse(row.get(1))?;
    Ok(SelectiveInvalidationPlan {
        provider_work_id: row.get(0),
        change,
        reevaluate: json_string_vec(row.get(2))?,
        leave_valid: json_string_vec(row.get(3))?,
        unaffected_work_ids: json_string_vec(row.get(4))?,
    })
}

async fn load_boundary(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    check_id: &str,
) -> PgResult<BoundaryRevalidation> {
    let row = tx
        .query_one(
            "SELECT decision, recovery_duty, erase_effects, unrelated_may_continue, reason
             FROM awr_team.execution_boundary_checks
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &check_id],
        )
        .await?;
    let decision = match row.get::<_, String>(0).as_str() {
        "allow" => BoundaryDecision::Allow,
        "block_new_effects" => BoundaryDecision::BlockNewEffects,
        "keep_effects_assign_recovery" => BoundaryDecision::KeepEffectsAssignRecovery,
        _ => return Err(PgError::Protocol("unknown boundary decision".into())),
    };
    Ok(BoundaryRevalidation {
        decision,
        recovery_duty: row.get(1),
        erase_effects: row.get(2),
        unrelated_may_continue: row.get(3),
        reason: row.get(4),
    })
}

async fn load_planning_change(
    tx: &Transaction<'_>,
    tenant: &str,
    project: &str,
    change_id: &str,
) -> PgResult<ScopedPlanningChange> {
    let row = tx
        .query_one(
            "SELECT id, discovered_by, old_graph_version, new_graph_version,
                    old_acceptance_contract, new_acceptance_contract,
                    affected_json, cancel_split_json, continue_conditions_json,
                    status, confirmed_by
             FROM awr_team.scoped_planning_changes
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&tenant, &project, &change_id],
        )
        .await?;
    let cancel: Value = row.get(7);
    let relations: Vec<CancelSplitRelation> = serde_json::from_value(cancel)
        .map_err(|e| PgError::Protocol(format!("cancel_split decode: {e}")))?;
    Ok(ScopedPlanningChange {
        change_id: row.get(0),
        discovered_by: row.get(1),
        old_graph_version: row.get(2),
        new_graph_version: row.get(3),
        old_acceptance_contract: row.get(4),
        new_acceptance_contract: row.get(5),
        affected_work_ids: json_string_vec(row.get(6))?,
        cancel_split_relations: relations,
        continue_conditions: json_string_vec(row.get(8))?,
        status: row.get(9),
        confirmed_by: row.get(10),
    })
}

fn json_string_vec(v: Value) -> PgResult<Vec<String>> {
    serde_json::from_value(v).map_err(|e| PgError::Protocol(format!("json string vec: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_survives_progress_current_reevaluates() {
        let consumers = vec![
            AdoptedConsumerEdge {
                dependency_id: "d1".into(),
                consumer_work_id: "sdk".into(),
                provider_work_id: "api".into(),
                policy: "fixed_delivery".into(),
                credential_status: "active".into(),
                assessment_status: "satisfied".into(),
            },
            AdoptedConsumerEdge {
                dependency_id: "d2".into(),
                consumer_work_id: "integration".into(),
                provider_work_id: "api".into(),
                policy: "current_contract".into(),
                credential_status: "active".into(),
                assessment_status: "satisfied".into(),
            },
        ];
        let plan = select_downstream_reevaluation(
            "api",
            ProviderChangeKind::NewVersionOrProgress,
            &consumers,
            &[
                "api".into(),
                "sdk".into(),
                "integration".into(),
                "docs".into(),
            ],
        );
        assert_eq!(plan.reevaluate, vec!["integration".to_string()]);
        assert_eq!(plan.leave_valid, vec!["sdk".to_string()]);
        assert_eq!(plan.unaffected_work_ids, vec!["docs".to_string()]);
    }

    #[test]
    fn mid_execution_keeps_effects() {
        let v = revalidate_execution_boundary(&BoundarySnapshot {
            boundary: ExecutionBoundary::Complete,
            work_id: "sdk".into(),
            execution_id: Some("ex1".into()),
            dependencies_satisfied: false,
            revoke_or_invalidation_observed: true,
            has_real_effects: true,
            unrelated_work_ids: vec!["docs".into()],
        });
        assert_eq!(v.decision, BoundaryDecision::KeepEffectsAssignRecovery);
        assert!(v.recovery_duty);
        assert!(!v.erase_effects);
        assert!(v.unrelated_may_continue);
    }
}
