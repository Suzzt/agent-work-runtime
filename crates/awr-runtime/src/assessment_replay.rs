//! DEC-022: offline assessment replay from frozen snapshots.
//!
//! Recomputes explanation/assessment envelopes from fixed inputs only.
//! Never re-reads production store state, never re-runs tools, never issues
//! model or network requests. Missing snapshots are explicitly not replayable.
//! Snapshot JSON is artifact-shaped (reuse event/artifact stores; no separate DB).

use crate::explanation_chain::{
    CompletionExplanationInput, DeliveryExplanationInput, ExplanationAuthority,
    ExplanationChainInput, ExplanationChainResult, compose_explanation_chain,
};
use crate::fact_snapshot::PreparedFactView;
use awr_core::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Artifact / event payload schema for a bounded offline replay snapshot.
pub const REPLAY_SNAPSHOT_SCHEMA_ID: &str = "awr-assessment-replay-snapshot-v1";
pub const REPLAY_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Capability catalog id for developer offline replay.
pub const ASSESSMENT_REPLAY_CAPABILITY: &str = "assessment.replay";

/// Frozen inputs sufficient to recompute an explanation chain offline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaySnapshot {
    pub schema_id: String,
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_ref: Option<String>,
    pub prepared: PreparedFactView,
    pub policy: AssessmentPolicy,
    pub rule_hash: String,
    pub as_of: i64,
    #[serde(default)]
    pub delivery: DeliveryExplanationInput,
    #[serde(default)]
    pub completion: CompletionExplanationInput,
    #[serde(default)]
    pub current_authority: ExplanationAuthority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_identity: Option<AssessmentIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_assessment_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayStatus {
    Replayed,
    NotReplayable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReport {
    pub status: ReplayStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub offline: bool,
    pub reread_production_state: bool,
    pub reran_tools: bool,
    pub model_or_network_requests: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_hash_matched: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<AssessmentEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_explanation_valid: Option<bool>,
    pub query_original_only: bool,
}

impl ReplaySnapshot {
    pub fn validate_for_replay(&self) -> Result<()> {
        if self.schema_id != REPLAY_SNAPSHOT_SCHEMA_ID {
            return Err(Error::InvalidInput(format!(
                "replay snapshot schema_id must be {REPLAY_SNAPSHOT_SCHEMA_ID}"
            )));
        }
        if self.schema_version != REPLAY_SNAPSHOT_SCHEMA_VERSION {
            return Err(Error::InvalidInput(format!(
                "replay snapshot schema_version must be {REPLAY_SNAPSHOT_SCHEMA_VERSION}"
            )));
        }
        if self.prepared.work_key.trim().is_empty() {
            return Err(Error::InvalidInput(
                "replay snapshot prepared.work_key must be nonempty".into(),
            ));
        }
        if self.rule_hash.trim().is_empty() {
            return Err(Error::InvalidInput(
                "replay snapshot rule_hash must be nonempty".into(),
            ));
        }
        let expected = rule_hash_for_policy(&self.policy)?;
        if self.rule_hash != expected {
            return Err(Error::InvalidInput(format!(
                "replay snapshot rule_hash does not match embedded policy (have {}, expected {})",
                self.rule_hash, expected
            )));
        }
        let _ = self.policy.clone().capped()?;
        Ok(())
    }
}

/// Canonical rule hash for an assessment policy (artifact-bound, no separate DB).
pub fn rule_hash_for_policy(policy: &AssessmentPolicy) -> Result<String> {
    let body = serde_json::to_vec(&policy.clone().capped()?)?;
    let mut hasher = Sha256::new();
    hasher.update(b"awr-assessment-rule-hash-v1\0");
    hasher.update(&body);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Build a snapshot from fixed chain inputs (caller supplies already-frozen facts).
pub fn capture_replay_snapshot(
    prepared: PreparedFactView,
    policy: AssessmentPolicy,
    as_of: i64,
    delivery: DeliveryExplanationInput,
    completion: CompletionExplanationInput,
    current_authority: ExplanationAuthority,
    prior_identity: Option<AssessmentIdentity>,
    recorded_assessment_hash: Option<String>,
    artifact_ref: Option<String>,
    event_ref: Option<String>,
) -> Result<ReplaySnapshot> {
    let policy = policy.capped()?;
    let rule_hash = rule_hash_for_policy(&policy)?;
    let snap = ReplaySnapshot {
        schema_id: REPLAY_SNAPSHOT_SCHEMA_ID.into(),
        schema_version: REPLAY_SNAPSHOT_SCHEMA_VERSION,
        artifact_ref,
        event_ref,
        prepared,
        policy,
        rule_hash,
        as_of,
        delivery,
        completion,
        current_authority,
        prior_identity,
        recorded_assessment_hash,
    };
    snap.validate_for_replay()?;
    Ok(snap)
}

fn not_replayable(reason: impl Into<String>) -> ReplayReport {
    ReplayReport {
        status: ReplayStatus::NotReplayable,
        reason: Some(reason.into()),
        offline: true,
        reread_production_state: false,
        reran_tools: false,
        model_or_network_requests: false,
        assessment_hash: None,
        rule_hash: None,
        policy_id: None,
        policy_version: None,
        recorded_hash_matched: None,
        envelope: None,
        prior_explanation_valid: None,
        query_original_only: false,
    }
}

/// Explicit refusal when the original snapshot is absent.
pub fn replay_missing_snapshot() -> ReplayReport {
    not_replayable("original snapshot missing; assessment is not replayable")
}

/// Offline replay: recompute from the frozen snapshot only.
pub fn replay_assessment(snapshot: &ReplaySnapshot) -> Result<ReplayReport> {
    if let Err(err) = snapshot.validate_for_replay() {
        return Ok(not_replayable(err.to_string()));
    }
    let result = compose_explanation_chain(ExplanationChainInput {
        prepared: snapshot.prepared.clone(),
        management_decision: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        delivery: snapshot.delivery.clone(),
        completion: snapshot.completion.clone(),
        current_authority: snapshot.current_authority.clone(),
        prior_identity: snapshot.prior_identity.clone(),
        policy: snapshot.policy.clone(),
        as_of: snapshot.as_of,
    })?;
    Ok(replay_report_from_result(snapshot, result))
}

fn replay_report_from_result(
    snapshot: &ReplaySnapshot,
    result: ExplanationChainResult,
) -> ReplayReport {
    let hash = result.envelope.assessment_hash.clone();
    let matched = snapshot
        .recorded_assessment_hash
        .as_ref()
        .map(|recorded| recorded == &hash);
    ReplayReport {
        status: ReplayStatus::Replayed,
        reason: None,
        offline: true,
        reread_production_state: false,
        reran_tools: false,
        model_or_network_requests: false,
        assessment_hash: Some(hash),
        rule_hash: Some(snapshot.rule_hash.clone()),
        policy_id: Some(snapshot.policy.policy_id.clone()),
        policy_version: Some(snapshot.policy.policy_version),
        recorded_hash_matched: matched,
        prior_explanation_valid: result.prior_explanation_valid,
        query_original_only: result.query_original_only,
        envelope: Some(result.envelope),
    }
}

/// Parse snapshot JSON from an artifact/event payload (or developer file).
pub fn parse_replay_snapshot(bytes: &[u8]) -> Result<ReplaySnapshot> {
    if bytes.is_empty() {
        return Err(Error::InvalidInput(
            "original snapshot missing; assessment is not replayable".into(),
        ));
    }
    let snap: ReplaySnapshot = serde_json::from_slice(bytes).map_err(|e| {
        Error::InvalidInput(format!(
            "original snapshot missing or malformed; assessment is not replayable: {e}"
        ))
    })?;
    snap.validate_for_replay()?;
    Ok(snap)
}

/// Load + replay helper for CLI/MCP developer entry points.
pub fn replay_assessment_from_bytes(bytes: Option<&[u8]>) -> Result<ReplayReport> {
    let Some(bytes) = bytes else {
        return Ok(replay_missing_snapshot());
    };
    if bytes.is_empty() {
        return Ok(replay_missing_snapshot());
    }
    match parse_replay_snapshot(bytes) {
        Ok(snap) => replay_assessment(&snap),
        Err(err) => Ok(not_replayable(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_view() -> PreparedFactView {
        PreparedFactView {
            project_id: Some("01PROJ".into()),
            work_key: "AWR-DEC-022".into(),
            work_id: Some("01WORK".into()),
            branch_id: Some("main".into()),
            work_revision: Some("1".into()),
            source_revision: Some("900".into()),
            work_contract_hash: Some("contract-a".into()),
            as_of: 42,
            ready: Some(true),
            diagnostics: vec![],
            active_claim_ids: vec![],
            management: Some(json!({
                "contract_fingerprint": "contract-a",
                "decision": {
                    "version": 1,
                    "mode": "lightweight",
                    "reasons": [],
                    "unknown_observations": [],
                    "reevaluation_signals": [],
                    "required_actions": [
                        "preserve_identity_intent_scope_and_current_state",
                        "consume_required_context_and_hard_rules",
                        "retain_completion_basis_and_actual_outcome",
                        "check_source_versions_permissions_claims_and_request_identity"
                    ],
                    "optional_maintenance": [],
                    "completion_policy": "unchanged_source_policy",
                    "execution_admission": "not_granted_by_management_classification"
                },
                "observation_basis": "host_assertion_not_independently_verified",
                "record_required": false,
                "admission_gaps": [],
                "next_action": "follow_required_actions_and_existing_workflow"
            })),
            context_complete: Some(true),
            context_issues: vec![],
            source_refs: vec!["source:ledger".into()],
            runtime_observation_refs: vec![],
            host_observation: None,
            workspace_facts: None,
            limits: None,
        }
    }

    fn sample_snapshot() -> ReplaySnapshot {
        capture_replay_snapshot(
            sample_view(),
            AssessmentPolicy::default(),
            42,
            DeliveryExplanationInput::default(),
            CompletionExplanationInput::default(),
            ExplanationAuthority {
                contract_hash: Some("contract-a".into()),
                source_revision: Some("900".into()),
                auth_fingerprint: Some("auth-ok".into()),
                stop_or_revoke: false,
            },
            None,
            None,
            Some("artifact:replay-demo".into()),
            None,
        )
        .expect("capture")
    }

    #[test]
    fn missing_snapshot_is_explicitly_not_replayable() {
        let report = replay_assessment_from_bytes(None).unwrap();
        assert_eq!(report.status, ReplayStatus::NotReplayable);
        assert!(report.reason.as_deref().unwrap().contains("not replayable"));
        assert!(report.offline);
        assert!(!report.reread_production_state);
        assert!(!report.reran_tools);
        assert!(!report.model_or_network_requests);
    }

    #[test]
    fn replay_recomputes_fixed_inputs_without_external_io() {
        let snap = sample_snapshot();
        let a = replay_assessment(&snap).unwrap();
        let b = replay_assessment(&snap).unwrap();
        assert_eq!(a.status, ReplayStatus::Replayed);
        assert_eq!(a.assessment_hash, b.assessment_hash);
        assert!(a.offline);
        assert!(!a.reread_production_state);
        assert!(!a.reran_tools);
        assert!(!a.model_or_network_requests);
        assert_eq!(a.rule_hash.as_deref(), Some(snap.rule_hash.as_str()));
    }
}
