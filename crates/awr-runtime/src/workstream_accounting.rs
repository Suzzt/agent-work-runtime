//! Independent scope and delivery accounting (WS-040).
//!
//! Thin runtime façade over `awr_core::workstream_accounting`. Callers must
//! authenticate the project, load an approved immutable contract snapshot, and
//! verify evidence provenance / applicability / revocation before constructing
//! [`VerifiedStageObservation`] values. This module:
//! - freezes the required-work denominator from that versioned contract;
//! - keeps plan / implement / verify / merge / release as independent stages;
//! - never treats a Goal query hit-list as contract completion rate;
//! - preserves unique ownership, shared-outcome references, and transfer history
//!   without double-counting or silently rewriting past contracts.
use awr_core::{
    AccountingContractIdentity, AccountingError, AccountingEvidence, AccountingStage,
    AccountingStages, AccountingWork, Id, WORKSTREAM_ACCOUNTING_VERSION, WorkstreamAccounting,
    WorkstreamAccountingContract, WorkstreamWorkBinding, account_workstream,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

/// Adapter attestation that the project and contract bytes were authorized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedContractSnapshot {
    pub project_id: String,
    pub contract: WorkstreamAccountingContract,
    /// True only after the adapter verified approval and digest match.
    pub approval_attested: bool,
}

/// Provenance-checked stage observation. Untrusted / revoked never become Recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifiedStageObservation {
    Unknown,
    NotMet,
    /// Provenance failed closed — not counted as Recorded.
    Untrusted,
    /// Explicitly revoked — not counted as Recorded and not erasable history.
    Revoked,
    Recorded(AccountingEvidence),
}

impl VerifiedStageObservation {
    fn into_stage(self) -> AccountingStage {
        match self {
            Self::Unknown | Self::Untrusted | Self::Revoked => AccountingStage::Unknown,
            Self::NotMet => AccountingStage::NotMet,
            Self::Recorded(evidence) => AccountingStage::Recorded(evidence),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedWorkObservation {
    pub binding: WorkstreamWorkBinding,
    /// Source "done" is retained but never promotes any stage.
    pub source_declared_done: bool,
    pub planned: VerifiedStageObservation,
    pub implemented: VerifiedStageObservation,
    pub verified: VerifiedStageObservation,
    pub merged: VerifiedStageObservation,
    pub released: VerifiedStageObservation,
}

/// Goal filter hits — intentionally separate from contract accounting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalQueryResult {
    pub goal_key: String,
    pub work_item_ids: Vec<String>,
}

/// A Goal query view that must not be presented as contract completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalQueryView {
    pub goal_key: String,
    pub hit_count: usize,
    /// Always true: adapters must not coerce this into a completion percent.
    pub is_not_contract_completion_rate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractAccountingReport {
    pub accounting: WorkstreamAccounting,
    pub goal_query: Option<GoalQueryView>,
    /// Stages remain independent; no single blended completion percent is emitted.
    pub stages_are_independent: bool,
    pub shared_outcomes_not_owned_achievements: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipTransfer {
    pub work_item_id: String,
    pub from_workstream_id: Id,
    pub to_workstream_id: Id,
    /// Required: transfers need a new approved revision, never an in-place rewrite.
    pub new_revision: u64,
    pub new_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferAccountingOutcome {
    pub historical: WorkstreamAccounting,
    pub historical_contract: AccountingContractIdentity,
    pub successor_identity: AccountingContractIdentity,
    pub transferred_work_item_id: String,
    pub history_rewritten: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScopeAccountingError {
    #[error("project is not attested for this contract")]
    ProjectNotAttested,
    #[error("approved contract project does not match authenticated project")]
    ProjectMismatch,
    #[error("approval attestation missing for the selected contract")]
    ApprovalRequired,
    #[error("goal query must not replace the frozen contract denominator")]
    GoalQueryMasquerade,
    #[error("ownership transfer requires a new contract revision and digest")]
    TransferRequiresNewContract,
    #[error("silent rewrite of historical ownership is refused")]
    HistoryRewriteRefused,
    #[error("shared outcome cannot also be an owned achievement of this stream")]
    SharedOutcomeDoubleCount,
    #[error(transparent)]
    Accounting(#[from] AccountingError),
}

fn map_rows(
    snapshot: &ApprovedContractSnapshot,
    observations: &[VerifiedWorkObservation],
) -> Result<Vec<AccountingWork>, ScopeAccountingError> {
    let identity = &snapshot.contract.identity;
    Ok(observations
        .iter()
        .map(|obs| AccountingWork {
            binding: obs.binding.clone(),
            contract: identity.clone(),
            source_declared_done: obs.source_declared_done,
            stages: AccountingStages {
                planned: obs.planned.clone().into_stage(),
                implemented: obs.implemented.clone().into_stage(),
                verified: obs.verified.clone().into_stage(),
                merged: obs.merged.clone().into_stage(),
                released: obs.released.clone().into_stage(),
            },
        })
        .collect())
}

fn assert_approved(snapshot: &ApprovedContractSnapshot) -> Result<(), ScopeAccountingError> {
    if snapshot.project_id.trim().is_empty() {
        return Err(ScopeAccountingError::ProjectNotAttested);
    }
    if !snapshot.approval_attested {
        return Err(ScopeAccountingError::ApprovalRequired);
    }
    if snapshot.contract.identity.project_id != snapshot.project_id {
        return Err(ScopeAccountingError::ProjectMismatch);
    }
    if snapshot.contract.version != WORKSTREAM_ACCOUNTING_VERSION {
        return Err(ScopeAccountingError::Accounting(
            AccountingError::UnsupportedVersion,
        ));
    }
    Ok(())
}

/// Goal hits are a filter view only — never a contract completion rate.
pub fn goal_query_view(goal: &GoalQueryResult) -> GoalQueryView {
    GoalQueryView {
        goal_key: goal.goal_key.clone(),
        hit_count: goal.work_item_ids.iter().collect::<BTreeSet<_>>().len(),
        is_not_contract_completion_rate: true,
    }
}

/// Refuse to treat Goal query cardinality as the required-work denominator.
pub fn refuse_goal_query_as_contract_rate(
    goal: &GoalQueryResult,
    contract: &WorkstreamAccountingContract,
) -> Result<(), ScopeAccountingError> {
    let hits: BTreeSet<_> = goal.work_item_ids.iter().collect();
    // Any attempt to expand or replace the frozen set via Goal hits is refused.
    if hits.len() != contract.required_work.len()
        || contract
            .required_work
            .iter()
            .any(|b| !hits.contains(&b.work_item_id))
    {
        return Err(ScopeAccountingError::GoalQueryMasquerade);
    }
    // Even an exact set match must not be used as a rate; callers use account_approved_scope.
    Err(ScopeAccountingError::GoalQueryMasquerade)
}

/// Account an approved, versioned contract. Goal query is retained as a labeled
/// side channel and never becomes `required_count` or a blended completion %.
pub fn account_approved_scope(
    snapshot: &ApprovedContractSnapshot,
    observations: &[VerifiedWorkObservation],
    goal_query: Option<&GoalQueryResult>,
) -> Result<ContractAccountingReport, ScopeAccountingError> {
    assert_approved(snapshot)?;
    // Shared references must not also appear as required owned work.
    let required: BTreeSet<_> = snapshot
        .contract
        .required_work
        .iter()
        .map(|b| b.work_item_id.as_str())
        .collect();
    for shared in &snapshot.contract.shared_references {
        if required.contains(shared.work_item_id.as_str()) {
            return Err(ScopeAccountingError::SharedOutcomeDoubleCount);
        }
    }
    let rows = map_rows(snapshot, observations)?;
    let accounting = account_workstream(&snapshot.contract, &rows)?;
    Ok(ContractAccountingReport {
        accounting,
        goal_query: goal_query.map(goal_query_view),
        stages_are_independent: true,
        shared_outcomes_not_owned_achievements: true,
    })
}

/// Preserve historical attribution under the frozen contract; a transfer only
/// proceeds with a successor identity (new revision + digest). Mutating past
/// bindings in place is refused.
pub fn transfer_work_preserving_history(
    historical: &ApprovedContractSnapshot,
    historical_observations: &[VerifiedWorkObservation],
    transfer: &OwnershipTransfer,
) -> Result<TransferAccountingOutcome, ScopeAccountingError> {
    assert_approved(historical)?;
    if transfer.new_revision <= historical.contract.identity.revision
        || transfer.new_digest.trim().is_empty()
        || transfer.new_digest == historical.contract.identity.digest
        || transfer.from_workstream_id == transfer.to_workstream_id
        || u128::from(transfer.to_workstream_id) == 0
    {
        return Err(ScopeAccountingError::TransferRequiresNewContract);
    }
    let owned = historical
        .contract
        .required_work
        .iter()
        .find(|b| b.work_item_id == transfer.work_item_id)
        .ok_or(ScopeAccountingError::Accounting(
            AccountingError::WorkSetMismatch,
        ))?;
    if owned.workstream_id != transfer.from_workstream_id {
        return Err(ScopeAccountingError::Accounting(
            AccountingError::BindingMismatch,
        ));
    }
    // Historical ledger must still account cleanly under the old identity.
    let historical_report = account_approved_scope(historical, historical_observations, None)?;

    // Silent rewrite: same identity, mutated ownership bindings — refused.
    let mut rewritten = historical.contract.clone();
    if let Some(binding) = rewritten
        .required_work
        .iter_mut()
        .find(|b| b.work_item_id == transfer.work_item_id)
    {
        binding.workstream_id = transfer.to_workstream_id;
    }
    let rewrite_rows = map_rows(historical, historical_observations)?;
    match account_workstream(&rewritten, &rewrite_rows) {
        Err(AccountingError::BindingMismatch) | Err(AccountingError::ContractMismatch) => {}
        Ok(_) => return Err(ScopeAccountingError::HistoryRewriteRefused),
        Err(other) => {
            // Any other failure still means the silent rewrite did not succeed as current delivery.
            let _ = other;
        }
    }

    let successor_identity = AccountingContractIdentity {
        project_id: historical.contract.identity.project_id.clone(),
        workstream_id: historical.contract.identity.workstream_id,
        contract_id: historical.contract.identity.contract_id.clone(),
        revision: transfer.new_revision,
        digest: transfer.new_digest.clone(),
    };

    Ok(TransferAccountingOutcome {
        historical: historical_report.accounting,
        historical_contract: historical.contract.identity.clone(),
        successor_identity,
        transferred_work_item_id: transfer.work_item_id.clone(),
        history_rewritten: false,
    })
}

/// Unique ownership keys across required bindings (one owner per work item).
pub fn unique_owned_work_keys(
    ownership: &[WorkstreamWorkBinding],
) -> Result<Vec<String>, ScopeAccountingError> {
    let mut seen = BTreeSet::new();
    let mut keys = Vec::new();
    for binding in ownership {
        if !seen.insert(binding.work_item_id.clone()) {
            return Err(ScopeAccountingError::Accounting(
                AccountingError::DuplicateWork,
            ));
        }
        keys.push(binding.work_item_id.clone());
    }
    keys.sort();
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> AccountingContractIdentity {
        AccountingContractIdentity {
            project_id: "project".into(),
            workstream_id: Id::from(1),
            contract_id: "contract".into(),
            revision: 1,
            digest: "digest-v1".into(),
        }
    }

    fn binding(work: &str, stream: u128) -> WorkstreamWorkBinding {
        WorkstreamWorkBinding {
            project_id: "project".into(),
            workstream_id: Id::from(stream),
            work_item_id: work.into(),
        }
    }

    fn recorded(b: &WorkstreamWorkBinding) -> VerifiedStageObservation {
        VerifiedStageObservation::Recorded(AccountingEvidence {
            reference: "receipt".into(),
            source_version: "source-v1".into(),
            environment: "candidate".into(),
            acceptance_round: "round-1".into(),
            occurred_at_ms: 10,
            attribution: b.clone(),
        })
    }

    fn snapshot(required: &[&str], shared: &[(&str, u128)]) -> ApprovedContractSnapshot {
        let id = identity();
        ApprovedContractSnapshot {
            project_id: "project".into(),
            approval_attested: true,
            contract: WorkstreamAccountingContract {
                version: WORKSTREAM_ACCOUNTING_VERSION,
                identity: id.clone(),
                required_work: required.iter().map(|w| binding(w, 1)).collect(),
                shared_references: shared.iter().map(|(w, s)| binding(w, *s)).collect(),
            },
        }
    }

    fn blank_obs(b: WorkstreamWorkBinding) -> VerifiedWorkObservation {
        VerifiedWorkObservation {
            binding: b,
            source_declared_done: false,
            planned: VerifiedStageObservation::Unknown,
            implemented: VerifiedStageObservation::Unknown,
            verified: VerifiedStageObservation::Unknown,
            merged: VerifiedStageObservation::Unknown,
            released: VerifiedStageObservation::Unknown,
        }
    }

    #[test]
    fn freezes_denominator_and_keeps_stages_independent() {
        let snap = snapshot(&["a", "b"], &[]);
        let mut rows = vec![blank_obs(binding("a", 1)), blank_obs(binding("b", 1))];
        rows[0].source_declared_done = true;
        rows[0].planned = recorded(&rows[0].binding);
        rows[0].implemented = recorded(&rows[0].binding);
        rows[1].verified = VerifiedStageObservation::NotMet;
        rows[1].released = recorded(&rows[1].binding);
        let goal = GoalQueryResult {
            goal_key: "deliver".into(),
            // Goal query returns extras that must not expand the denominator.
            work_item_ids: vec!["a".into(), "b".into(), "planning-extra".into()],
        };
        let report = account_approved_scope(&snap, &rows, Some(&goal)).unwrap();
        assert_eq!(report.accounting.required_count, 2);
        assert_eq!(report.accounting.source_declared_done, 1);
        assert_eq!(report.accounting.planned.recorded, 1);
        assert_eq!(report.accounting.implemented.recorded, 1);
        assert_eq!(report.accounting.verified.not_met, 1);
        assert_eq!(report.accounting.verified.unknown, 1);
        assert_eq!(report.accounting.merged.unknown, 2);
        assert_eq!(report.accounting.released.recorded, 1);
        assert!(report.stages_are_independent);
        let gq = report.goal_query.unwrap();
        assert_eq!(gq.hit_count, 3);
        assert!(gq.is_not_contract_completion_rate);
        assert_ne!(gq.hit_count, report.accounting.required_count);
    }

    #[test]
    fn goal_query_cannot_masquerade_as_contract_rate() {
        let snap = snapshot(&["a", "b"], &[]);
        let goal = GoalQueryResult {
            goal_key: "deliver".into(),
            work_item_ids: vec!["a".into(), "b".into(), "extra".into()],
        };
        assert_eq!(
            refuse_goal_query_as_contract_rate(&goal, &snap.contract),
            Err(ScopeAccountingError::GoalQueryMasquerade)
        );
        // Exact match is still refused — Goal query is never the rate API.
        let exact = GoalQueryResult {
            goal_key: "deliver".into(),
            work_item_ids: vec!["a".into(), "b".into()],
        };
        assert_eq!(
            refuse_goal_query_as_contract_rate(&exact, &snap.contract),
            Err(ScopeAccountingError::GoalQueryMasquerade)
        );
        let view = goal_query_view(&goal);
        assert!(view.is_not_contract_completion_rate);
    }

    #[test]
    fn untrusted_or_revoked_evidence_does_not_become_recorded() {
        let snap = snapshot(&["a"], &[]);
        let mut row = blank_obs(binding("a", 1));
        row.verified = VerifiedStageObservation::Untrusted;
        row.merged = VerifiedStageObservation::Revoked;
        row.planned = recorded(&row.binding);
        let report = account_approved_scope(&snap, &[row], None).unwrap();
        assert_eq!(report.accounting.planned.recorded, 1);
        assert_eq!(report.accounting.verified.unknown, 1);
        assert_eq!(report.accounting.merged.unknown, 1);
    }

    #[test]
    fn shared_outcomes_are_not_owned_achievements_and_refuse_double_count() {
        let snap = snapshot(&["a"], &[("provider-work", 2)]);
        let row = blank_obs(binding("a", 1));
        let report = account_approved_scope(&snap, &[row], None).unwrap();
        assert_eq!(report.accounting.required_count, 1);
        assert_eq!(report.accounting.shared_reference_count, 1);
        assert!(report.shared_outcomes_not_owned_achievements);

        let mut bad = snap.clone();
        bad.contract.shared_references.push(binding("a", 2));
        assert_eq!(
            account_approved_scope(&bad, &[blank_obs(binding("a", 1))], None),
            Err(ScopeAccountingError::SharedOutcomeDoubleCount)
        );
    }

    #[test]
    fn unique_ownership_rejects_duplicates() {
        assert_eq!(
            unique_owned_work_keys(&[binding("a", 1), binding("b", 2)]).unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            unique_owned_work_keys(&[binding("a", 1), binding("a", 2)]),
            Err(ScopeAccountingError::Accounting(
                AccountingError::DuplicateWork
            ))
        );
    }

    #[test]
    fn task_transfer_preserves_history_and_requires_new_contract() {
        let snap = snapshot(&["a", "b"], &[]);
        let mut rows = vec![blank_obs(binding("a", 1)), blank_obs(binding("b", 1))];
        rows[0].verified = recorded(&rows[0].binding);
        let transfer = OwnershipTransfer {
            work_item_id: "a".into(),
            from_workstream_id: Id::from(1),
            to_workstream_id: Id::from(2),
            new_revision: 2,
            new_digest: "digest-after-transfer".into(),
        };
        let outcome = transfer_work_preserving_history(&snap, &rows, &transfer).unwrap();
        assert!(!outcome.history_rewritten);
        assert_eq!(outcome.historical.verified.recorded, 1);
        assert_eq!(outcome.historical_contract.revision, 1);
        assert_eq!(outcome.historical_contract.digest, "digest-v1");
        assert_eq!(outcome.successor_identity.revision, 2);
        assert_eq!(outcome.successor_identity.digest, "digest-after-transfer");
        // Historical ledger still accounts under the old contract.
        let again = account_approved_scope(&snap, &rows, None).unwrap();
        assert_eq!(again.accounting, outcome.historical);

        let same_rev = OwnershipTransfer {
            new_revision: 1,
            new_digest: "other".into(),
            ..transfer.clone()
        };
        assert_eq!(
            transfer_work_preserving_history(&snap, &rows, &same_rev),
            Err(ScopeAccountingError::TransferRequiresNewContract)
        );
    }

    #[test]
    fn approval_and_project_attestation_are_required() {
        let mut snap = snapshot(&["a"], &[]);
        snap.approval_attested = false;
        assert_eq!(
            account_approved_scope(&snap, &[blank_obs(binding("a", 1))], None),
            Err(ScopeAccountingError::ApprovalRequired)
        );
        snap.approval_attested = true;
        snap.project_id = "other".into();
        assert_eq!(
            account_approved_scope(&snap, &[blank_obs(binding("a", 1))], None),
            Err(ScopeAccountingError::ProjectMismatch)
        );
    }
}
