//! Selective downstream invalidation and execution-boundary revalidation (WS-032).
//!
//! Pure rules only: adapters supply trusted adoption assessments and execution
//! observations. Fixed-delivery consumers survive unrelated upstream progress;
//! current-contract consumers revalidate on authoritative selection drift.
//! Prepare/dispatch/complete rechecks prevent revoke races. Mid-execution
//! invalidation keeps real effects and recovery duty; unrelated work continues.
use awr_core::{AdoptionCredentialStatus, DeliveryStatus, DeliveryVersionPolicy};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Why a provider-side observation may force downstream re-evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderChangeKind {
    /// Newer planning / newer completion that does not revoke the prior pinned delivery.
    NewVersionOrProgress,
    /// Authoritative current selection drifted (contract or selected receipt).
    CurrentSelectionChanged,
    /// Pinned or current delivery acceptance/export was revoked.
    DeliveryRevoked,
    /// The exact pinned artifact became unavailable.
    PinnedArtifactUnavailable,
}

/// One adopted consumer edge considered for selective invalidation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptedConsumerEdge {
    pub dependency_id: String,
    pub consumer_work_id: String,
    pub provider_work_id: String,
    pub policy: DeliveryVersionPolicy,
    pub credential_status: AdoptionCredentialStatus,
    /// Latest trusted assessment for this edge (Satisfied / Stale / Revoked / …).
    pub assessment_status: DeliveryStatus,
}

/// Outcome of selective selection: which consumers must re-evaluate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectiveInvalidationPlan {
    pub provider_work_id: String,
    pub change: ProviderChangeKind,
    pub reevaluate: Vec<String>,
    pub leave_valid: Vec<String>,
    pub unaffected_work_ids: Vec<String>,
}

/// Decide which adopted consumers of `provider_work_id` must re-evaluate.
///
/// Fixed-delivery consumers are left valid on mere new versions/progress.
/// Current-contract consumers re-evaluate on selection drift or revoke events.
/// Unrelated works (not consumers of this provider) always continue.
pub fn select_downstream_reevaluation(
    provider_work_id: &str,
    change: ProviderChangeKind,
    consumers: &[AdoptedConsumerEdge],
    all_project_work_ids: impl IntoIterator<Item = impl AsRef<str>>,
) -> SelectiveInvalidationPlan {
    let mut reevaluate = BTreeSet::new();
    let mut leave_valid = BTreeSet::new();
    let mut consumer_ids = BTreeSet::new();

    for edge in consumers {
        if edge.provider_work_id != provider_work_id {
            continue;
        }
        consumer_ids.insert(edge.consumer_work_id.clone());
        if edge.credential_status == AdoptionCredentialStatus::Revoked
            || edge.assessment_status == DeliveryStatus::Revoked
        {
            // Already revoked credentials still surface for recovery bookkeeping.
            reevaluate.insert(edge.consumer_work_id.clone());
            continue;
        }
        let needs = match (edge.policy, change) {
            (DeliveryVersionPolicy::FixedDelivery, ProviderChangeKind::NewVersionOrProgress) => {
                false
            }
            (DeliveryVersionPolicy::FixedDelivery, ProviderChangeKind::CurrentSelectionChanged) => {
                false
            }
            (
                DeliveryVersionPolicy::FixedDelivery,
                ProviderChangeKind::DeliveryRevoked | ProviderChangeKind::PinnedArtifactUnavailable,
            ) => true,
            (
                DeliveryVersionPolicy::CurrentContract,
                ProviderChangeKind::NewVersionOrProgress
                | ProviderChangeKind::CurrentSelectionChanged
                | ProviderChangeKind::DeliveryRevoked
                | ProviderChangeKind::PinnedArtifactUnavailable,
            ) => true,
        };
        if needs {
            reevaluate.insert(edge.consumer_work_id.clone());
        } else {
            leave_valid.insert(edge.consumer_work_id.clone());
        }
    }

    let unaffected: Vec<String> = all_project_work_ids
        .into_iter()
        .map(|w| w.as_ref().to_string())
        .filter(|w| w != provider_work_id && !consumer_ids.contains(w))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    SelectiveInvalidationPlan {
        provider_work_id: provider_work_id.to_string(),
        change,
        reevaluate: reevaluate.into_iter().collect(),
        leave_valid: leave_valid.into_iter().collect(),
        unaffected_work_ids: unaffected,
    }
}

/// Execution boundary where adoption/bindings must be rechecked atomically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBoundary {
    Prepare,
    Dispatch,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryDecision {
    /// Snapshot still unlocks the action.
    Allow,
    /// Refuse new effects / final acceptance (revoke race or stale binding).
    BlockNewEffects,
    /// Execution already produced effects; keep them and assign recovery duty.
    KeepEffectsAssignRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundarySnapshot {
    pub boundary: ExecutionBoundary,
    pub work_id: String,
    /// True when every required adoption/binding is currently Satisfied+Active.
    pub dependencies_satisfied: bool,
    /// True when a concurrent revoke or binding invalidation was observed under lock.
    pub revoke_or_invalidation_observed: bool,
    /// True when the execution has already produced attributed external effects.
    pub has_real_effects: bool,
    /// Other project works that must keep running regardless of this decision.
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

/// Recheck prepare / dispatch / complete against a coherent action snapshot.
///
/// Revoke races at a boundary block new effects. Mid-execution invalidation
/// keeps real effects, sets recovery duty, and never erases history. Unrelated
/// tasks always remain free to continue.
pub fn revalidate_execution_boundary(snapshot: &BoundarySnapshot) -> BoundaryRevalidation {
    let unrelated_may_continue = true;
    if snapshot.dependencies_satisfied && !snapshot.revoke_or_invalidation_observed {
        return BoundaryRevalidation {
            decision: BoundaryDecision::Allow,
            recovery_duty: false,
            erase_effects: false,
            unrelated_may_continue,
            reason: "dependencies satisfied at boundary".into(),
        };
    }
    if snapshot.has_real_effects {
        return BoundaryRevalidation {
            decision: BoundaryDecision::KeepEffectsAssignRecovery,
            recovery_duty: true,
            erase_effects: false,
            unrelated_may_continue,
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
        unrelated_may_continue,
        reason: reason.into(),
    }
}

/// Scoped planning change recorded when implementation discovers new deps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningChangeStatus {
    /// Persisted; affected actions are not yet blocked (should not linger).
    Recorded,
    /// Affected actions blocked pending authorized confirmation.
    AffectedBlocked,
    /// Authorized person confirmed new graph + acceptance contract.
    Confirmed,
    /// Change rejected; prior graph remains authoritative.
    Rejected,
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
    pub project_id: String,
    pub discovered_by: String,
    pub old_graph_version: String,
    pub new_graph_version: String,
    pub old_acceptance_contract: String,
    pub new_acceptance_contract: String,
    pub affected_work_ids: Vec<String>,
    pub cancel_split_relations: Vec<CancelSplitRelation>,
    pub continue_conditions: Vec<String>,
    pub status: PlanningChangeStatus,
    pub confirmed_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverDependencyRequest {
    pub change_id: String,
    pub project_id: String,
    pub discovered_by: String,
    pub old_graph_version: String,
    pub new_graph_version: String,
    pub old_acceptance_contract: String,
    pub new_acceptance_contract: String,
    pub affected_work_ids: Vec<String>,
    pub cancel_split_relations: Vec<CancelSplitRelation>,
    pub continue_conditions: Vec<String>,
    /// All project works; used to prove unrelated tasks stay unblocked.
    pub all_project_work_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningChangeApplication {
    pub change: ScopedPlanningChange,
    pub blocked_work_ids: Vec<String>,
    pub unblocked_unrelated_work_ids: Vec<String>,
}

fn nonempty(s: &str) -> bool {
    !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control)
}

/// Persistable planning-change construction: records versions/relations and
/// immediately marks affected works blocked. Unrelated works stay free.
pub fn record_discovered_dependency_change(
    req: DiscoverDependencyRequest,
) -> Result<PlanningChangeApplication, String> {
    if !nonempty(&req.change_id)
        || !nonempty(&req.project_id)
        || !nonempty(&req.discovered_by)
        || !nonempty(&req.old_graph_version)
        || !nonempty(&req.new_graph_version)
        || !nonempty(&req.old_acceptance_contract)
        || !nonempty(&req.new_acceptance_contract)
    {
        return Err("bounded planning-change identity fields required".into());
    }
    if req.old_graph_version == req.new_graph_version
        && req.old_acceptance_contract == req.new_acceptance_contract
        && req.affected_work_ids.is_empty()
    {
        return Err("planning change must alter graph, contract, or affected set".into());
    }
    if req.affected_work_ids.is_empty() {
        return Err("discovered dependency must name an affected work set".into());
    }
    let affected: Vec<String> = req
        .affected_work_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let affected_set: BTreeSet<_> = affected.iter().cloned().collect();
    let unrelated: Vec<String> = req
        .all_project_work_ids
        .into_iter()
        .filter(|w| !affected_set.contains(w))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let change = ScopedPlanningChange {
        change_id: req.change_id,
        project_id: req.project_id,
        discovered_by: req.discovered_by,
        old_graph_version: req.old_graph_version,
        new_graph_version: req.new_graph_version,
        old_acceptance_contract: req.old_acceptance_contract,
        new_acceptance_contract: req.new_acceptance_contract,
        affected_work_ids: affected.clone(),
        cancel_split_relations: req.cancel_split_relations,
        continue_conditions: req.continue_conditions,
        status: PlanningChangeStatus::AffectedBlocked,
        confirmed_by: None,
    };
    Ok(PlanningChangeApplication {
        change,
        blocked_work_ids: affected,
        unblocked_unrelated_work_ids: unrelated,
    })
}

/// Authorized confirmation of the new graph + acceptance contract.
pub fn confirm_planning_change(
    mut change: ScopedPlanningChange,
    confirmer: impl Into<String>,
) -> Result<ScopedPlanningChange, String> {
    let confirmer = confirmer.into();
    if !nonempty(&confirmer) {
        return Err("authorized confirmer required".into());
    }
    if change.status != PlanningChangeStatus::AffectedBlocked {
        return Err("only blocked planning changes can be confirmed".into());
    }
    change.status = PlanningChangeStatus::Confirmed;
    change.confirmed_by = Some(confirmer);
    Ok(change)
}

/// Reject a blocked planning change; prior graph remains authoritative.
pub fn reject_planning_change(
    mut change: ScopedPlanningChange,
    rejector: impl Into<String>,
) -> Result<ScopedPlanningChange, String> {
    let rejector = rejector.into();
    if !nonempty(&rejector) {
        return Err("authorized rejector required".into());
    }
    if change.status != PlanningChangeStatus::AffectedBlocked {
        return Err("only blocked planning changes can be rejected".into());
    }
    change.status = PlanningChangeStatus::Rejected;
    change.confirmed_by = Some(rejector);
    Ok(change)
}

/// Whether a work action is blocked by any open (AffectedBlocked) planning change.
pub fn action_blocked_by_planning_changes(
    work_id: &str,
    open_changes: &[ScopedPlanningChange],
) -> Option<String> {
    open_changes.iter().find_map(|c| {
        if c.status == PlanningChangeStatus::AffectedBlocked
            && c.affected_work_ids.iter().any(|w| w == work_id)
        {
            Some(c.change_id.clone())
        } else {
            None
        }
    })
}

/// Index consumers by provider for batch invalidation planning.
pub fn consumers_by_provider(
    edges: &[AdoptedConsumerEdge],
) -> BTreeMap<String, Vec<AdoptedConsumerEdge>> {
    let mut map = BTreeMap::new();
    for edge in edges {
        map.entry(edge.provider_work_id.clone())
            .or_insert_with(Vec::new)
            .push(edge.clone());
    }
    map
}

/// Stable fingerprint helper for tests / adapters (not a security hash).
pub fn planning_change_fingerprint(change: &ScopedPlanningChange) -> String {
    format!(
        "{}:{}:{}:{}",
        change.change_id,
        change.old_graph_version,
        change.new_graph_version,
        change.status_str()
    )
}

impl ScopedPlanningChange {
    pub fn status_str(&self) -> &'static str {
        match self.status {
            PlanningChangeStatus::Recorded => "recorded",
            PlanningChangeStatus::AffectedBlocked => "affected_blocked",
            PlanningChangeStatus::Confirmed => "confirmed",
            PlanningChangeStatus::Rejected => "rejected",
        }
    }
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
}

impl ExecutionBoundary {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Dispatch => "dispatch",
            Self::Complete => "complete",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::DeliveryVersionPolicy;

    fn edge(
        dep: &str,
        consumer: &str,
        provider: &str,
        policy: DeliveryVersionPolicy,
    ) -> AdoptedConsumerEdge {
        AdoptedConsumerEdge {
            dependency_id: dep.into(),
            consumer_work_id: consumer.into(),
            provider_work_id: provider.into(),
            policy,
            credential_status: AdoptionCredentialStatus::Active,
            assessment_status: DeliveryStatus::Satisfied,
        }
    }

    #[test]
    fn fixed_consumers_survive_unrelated_upstream_progress() {
        let consumers = vec![
            edge("d1", "sdk", "api", DeliveryVersionPolicy::FixedDelivery),
            edge(
                "d2",
                "integration",
                "api",
                DeliveryVersionPolicy::CurrentContract,
            ),
        ];
        let plan = select_downstream_reevaluation(
            "api",
            ProviderChangeKind::NewVersionOrProgress,
            &consumers,
            ["api", "sdk", "integration", "docs"],
        );
        assert_eq!(plan.reevaluate, vec!["integration".to_string()]);
        assert_eq!(plan.leave_valid, vec!["sdk".to_string()]);
        assert_eq!(plan.unaffected_work_ids, vec!["docs".to_string()]);
    }

    #[test]
    fn revoke_forces_fixed_and_current_reevaluation() {
        let consumers = vec![
            edge("d1", "sdk", "api", DeliveryVersionPolicy::FixedDelivery),
            edge(
                "d2",
                "integration",
                "api",
                DeliveryVersionPolicy::CurrentContract,
            ),
        ];
        let plan = select_downstream_reevaluation(
            "api",
            ProviderChangeKind::DeliveryRevoked,
            &consumers,
            ["api", "sdk", "integration"],
        );
        assert_eq!(
            plan.reevaluate,
            vec!["integration".to_string(), "sdk".to_string()]
        );
        assert!(plan.leave_valid.is_empty());
    }

    #[test]
    fn prepare_blocks_revoke_race_without_erasing_effects() {
        let snap = BoundarySnapshot {
            boundary: ExecutionBoundary::Prepare,
            work_id: "sdk".into(),
            dependencies_satisfied: false,
            revoke_or_invalidation_observed: true,
            has_real_effects: false,
            unrelated_work_ids: vec!["docs".into()],
        };
        let v = revalidate_execution_boundary(&snap);
        assert_eq!(v.decision, BoundaryDecision::BlockNewEffects);
        assert!(!v.erase_effects);
        assert!(v.unrelated_may_continue);
    }

    #[test]
    fn mid_execution_keeps_effects_and_recovery_duty() {
        let snap = BoundarySnapshot {
            boundary: ExecutionBoundary::Dispatch,
            work_id: "sdk".into(),
            dependencies_satisfied: false,
            revoke_or_invalidation_observed: true,
            has_real_effects: true,
            unrelated_work_ids: vec!["docs".into()],
        };
        let v = revalidate_execution_boundary(&snap);
        assert_eq!(v.decision, BoundaryDecision::KeepEffectsAssignRecovery);
        assert!(v.recovery_duty);
        assert!(!v.erase_effects);
        assert!(v.unrelated_may_continue);
    }

    #[test]
    fn discovered_deps_block_affected_then_confirm() {
        let app = record_discovered_dependency_change(DiscoverDependencyRequest {
            change_id: "pc-1".into(),
            project_id: "project".into(),
            discovered_by: "agent-a".into(),
            old_graph_version: "g1".into(),
            new_graph_version: "g2".into(),
            old_acceptance_contract: "c1".into(),
            new_acceptance_contract: "c2".into(),
            affected_work_ids: vec!["sdk".into(), "integration".into()],
            cancel_split_relations: vec![CancelSplitRelation {
                kind: "split".into(),
                from_work_id: "sdk".into(),
                to_work_id: "sdk-auth".into(),
            }],
            continue_conditions: vec!["export-grant-present".into()],
            all_project_work_ids: vec![
                "api".into(),
                "sdk".into(),
                "integration".into(),
                "docs".into(),
            ],
        })
        .unwrap();
        assert_eq!(app.change.status, PlanningChangeStatus::AffectedBlocked);
        assert_eq!(
            app.blocked_work_ids,
            vec!["integration".to_string(), "sdk".to_string()]
        );
        assert_eq!(
            app.unblocked_unrelated_work_ids,
            vec!["api".to_string(), "docs".to_string()]
        );
        assert_eq!(
            action_blocked_by_planning_changes("sdk", &[app.change.clone()]),
            Some("pc-1".into())
        );
        assert_eq!(
            action_blocked_by_planning_changes("docs", &[app.change.clone()]),
            None
        );
        let confirmed = confirm_planning_change(app.change, "owner").unwrap();
        assert_eq!(confirmed.status, PlanningChangeStatus::Confirmed);
        assert_eq!(confirmed.confirmed_by.as_deref(), Some("owner"));
        assert_eq!(
            action_blocked_by_planning_changes("sdk", &[confirmed]),
            None
        );
    }
}
