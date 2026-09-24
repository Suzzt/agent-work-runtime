//! DEC-022 acceptance: offline replay, shadow compare, advice kill-switch.
use awr_core::*;
use awr_runtime::{
    AdviceDeliveryMode, AttachExplanationOptions, CompletionExplanationInput,
    DeliveryExplanationInput, EXPLANATION_CHAIN_PROFILE, ExplanationAuthority, PreparedFactView,
    ReplayStatus, apply_advice_delivery_mode, attach_according_to_advice_mode,
    capture_replay_snapshot, hard_protections_after_disable, parse_replay_snapshot,
    prior_explanation_still_valid, replay_assessment, replay_assessment_from_bytes, shadow_compare,
};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/assessment/replay")
}

fn sample_view(source_revision: &str, contract: &str, as_of: i64) -> PreparedFactView {
    PreparedFactView {
        project_id: Some("01PROJDEC022".into()),
        work_key: "AWR-DEC-022".into(),
        work_id: Some("01WORKDEC022".into()),
        branch_id: Some("main".into()),
        work_revision: Some("1".into()),
        source_revision: Some(source_revision.into()),
        work_contract_hash: Some(contract.into()),
        as_of,
        ready: Some(true),
        diagnostics: vec![],
        active_claim_ids: vec![],
        management: Some(json!({
            "contract_fingerprint": contract,
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

fn authority(source_revision: &str, contract: &str) -> ExplanationAuthority {
    ExplanationAuthority {
        contract_hash: Some(contract.into()),
        source_revision: Some(source_revision.into()),
        auth_fingerprint: Some("auth-ok".into()),
        stop_or_revoke: false,
    }
}

/// Acceptance 1: replay only recomputes fixed inputs; missing snapshot => not replayable.
#[test]
fn ac1_offline_replay_fixed_inputs_and_missing_snapshot() {
    let snap = capture_replay_snapshot(
        sample_view("900", "contract-a", 100),
        AssessmentPolicy::default(),
        100,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        authority("900", "contract-a"),
        None,
        None,
        Some("artifact:dec022-baseline".into()),
        Some("event:dec022-baseline".into()),
    )
    .expect("capture");

    // Round-trip outside the checkout. Rewriting the tracked fixture changes
    // bytes under core.autocrlf and fails the Windows source_unchanged gate.
    let scratch = std::env::temp_dir().join(format!("awr-dec022-baseline-{}.json", snap.rule_hash));
    fs::write(&scratch, serde_json::to_vec_pretty(&snap).unwrap()).unwrap();
    let again = parse_replay_snapshot(&fs::read(&scratch).unwrap()).unwrap();
    let _ = fs::remove_file(&scratch);

    let committed_path = fixtures_dir().join("baseline-snapshot.json");
    let committed_bytes = fs::read(&committed_path).unwrap();
    let committed = parse_replay_snapshot(&committed_bytes).unwrap();
    assert_eq!(
        replay_assessment(&committed).unwrap().status,
        ReplayStatus::Replayed
    );
    assert_eq!(fs::read(&committed_path).unwrap(), committed_bytes);
    let a = replay_assessment(&snap).unwrap();
    let b = replay_assessment(&again).unwrap();
    assert_eq!(a.status, ReplayStatus::Replayed);
    assert_eq!(a.assessment_hash, b.assessment_hash);
    assert!(a.offline);
    assert!(!a.reread_production_state);
    assert!(!a.reran_tools);
    assert!(!a.model_or_network_requests);

    let missing = replay_assessment_from_bytes(None).unwrap();
    assert_eq!(missing.status, ReplayStatus::NotReplayable);
    assert!(
        missing
            .reason
            .as_deref()
            .unwrap()
            .contains("not replayable")
    );
}

/// Acceptance 2: same-input baseline/candidate compare of reasons/advice/rejects/costs;
/// diffs explained by rule version; failed samples retained.
#[test]
fn ac2_shadow_compare_same_inputs_rule_version_and_retained_failures() {
    let view = sample_view("900", "contract-a", 100);
    let auth = authority("900", "contract-a");
    let baseline_policy = AssessmentPolicy::default();
    let mut candidate_policy = AssessmentPolicy::default();
    candidate_policy.policy_version = baseline_policy.policy_version + 1;
    candidate_policy.policy_hash = Some("candidate-rule-v2".into());

    let baseline = capture_replay_snapshot(
        view.clone(),
        baseline_policy,
        100,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        auth.clone(),
        None,
        None,
        Some("artifact:baseline".into()),
        None,
    )
    .unwrap();
    let candidate = capture_replay_snapshot(
        view,
        candidate_policy,
        100,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        auth,
        None,
        None,
        Some("artifact:candidate".into()),
        None,
    )
    .unwrap();

    let candidate_path = fixtures_dir().join("candidate-snapshot.json");
    let candidate_bytes = fs::read(&candidate_path).unwrap();
    let committed_candidate = parse_replay_snapshot(&candidate_bytes).unwrap();
    assert_eq!(
        committed_candidate.prepared.work_key,
        candidate.prepared.work_key
    );
    assert_eq!(fs::read(&candidate_path).unwrap(), candidate_bytes);

    assert_ne!(baseline.rule_hash, candidate.rule_hash);
    let report = shadow_compare(&baseline, &candidate).unwrap();
    assert!(report.same_inputs);
    assert!(!report.execution_adoption);
    assert!(!report.context_adoption);
    assert!(!report.background_daemon);
    for diff in &report.differences {
        assert!(
            diff.explained_by_rule_version.contains(&baseline.rule_hash)
                && diff
                    .explained_by_rule_version
                    .contains(&candidate.rule_hash),
            "diff must cite rule versions: {diff:?}"
        );
    }
    if !report.differences.is_empty() {
        assert!(
            !report.retained_failure_samples.is_empty(),
            "divergent samples must be retained in full"
        );
        // Full envelopes present in retained sample.
        let sample = &report.retained_failure_samples[0];
        assert!(sample.get("baseline_envelope").is_some() || sample.get("differences").is_some());
    }
}

/// Acceptance 3: shadow does not adopt execution/context; no daemon; disable restores
/// prior advice only and keeps hard protections.
#[test]
fn ac3_shadow_and_killswitch_preserve_hard_protections() {
    let shadow = apply_advice_delivery_mode(AdviceDeliveryMode::Shadow);
    assert!(shadow.new_advice_attached);
    assert!(!shadow.execution_adoption_changed);
    assert!(!shadow.context_adoption_changed);
    assert!(!shadow.background_daemon);
    assert!(!AdviceDeliveryMode::Shadow.adopts_for_execution_or_context());

    let disabled = apply_advice_delivery_mode(AdviceDeliveryMode::Disabled);
    assert!(disabled.restores_prior_advice_behavior);
    assert!(!disabled.new_advice_attached);
    assert!(!disabled.background_daemon);
    assert!(disabled.hard_protections.claim_guards_active);
    assert!(disabled.hard_protections.completion_guards_active);
    assert!(disabled.hard_protections.admission_guards_active);
    for (_k, active) in hard_protections_after_disable() {
        assert!(active);
    }

    let receipt = json!({
        "work": {"external_key": "AWR-DEC-022", "id": "01WORKDEC022", "revision": 1},
        "ready": true,
        "diagnostics": [],
        "active_claims": [],
        "management": {
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
        }
    });
    let shadowed = attach_according_to_advice_mode(
        receipt.clone(),
        AdviceDeliveryMode::Shadow,
        AttachExplanationOptions {
            enabled: true,
            as_of: Some(100),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        shadowed["assessment_explanation"]["execution_adoption"],
        false
    );
    assert_eq!(
        shadowed["assessment_explanation"]["context_adoption"],
        false
    );
    assert_eq!(shadowed["assessment_explanation"]["shadow"], true);

    let killed = attach_according_to_advice_mode(
        receipt,
        AdviceDeliveryMode::Disabled,
        AttachExplanationOptions {
            enabled: true,
            as_of: Some(100),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(killed.get("assessment_explanation").is_none());
    assert_eq!(
        killed["advice_mode_effect"]["restores_prior_advice_behavior"],
        true
    );
    assert_eq!(
        killed["advice_mode_effect"]["hard_protections"]["claim_guards_active"],
        true
    );
}

/// Real offline chain: prepare/explain → change source → reject stale → reassess.
#[test]
fn offline_chain_prepare_explain_source_change_reject_stale_reassess() {
    // 1) Prepare + explain on frozen inputs (capture snapshot).
    let prepared = sample_view("900", "contract-a", 100);
    let auth = authority("900", "contract-a");
    let snap = capture_replay_snapshot(
        prepared.clone(),
        AssessmentPolicy::default(),
        100,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        auth.clone(),
        None,
        None,
        Some("artifact:chain-step1".into()),
        None,
    )
    .unwrap();
    let explained = replay_assessment(&snap).unwrap();
    assert_eq!(explained.status, ReplayStatus::Replayed);
    let prior_identity = explained
        .envelope
        .as_ref()
        .map(|e| e.identity.clone())
        .expect("envelope");
    assert_eq!(
        explained
            .envelope
            .as_ref()
            .unwrap()
            .assessment_profile
            .as_str(),
        EXPLANATION_CHAIN_PROFILE
    );

    // 2) Source/contract change.
    let changed_auth = ExplanationAuthority {
        contract_hash: Some("contract-b".into()),
        source_revision: Some("901".into()),
        auth_fingerprint: Some("auth-ok".into()),
        stop_or_revoke: false,
    };

    // 3) Reject stale advice.
    assert!(
        !prior_explanation_still_valid(&prior_identity, &changed_auth),
        "stale advice after source/contract change must be rejected"
    );

    // 4) Reassess on new frozen inputs (still offline).
    let reassess_snap = capture_replay_snapshot(
        sample_view("901", "contract-b", 101),
        AssessmentPolicy::default(),
        101,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        changed_auth,
        Some(prior_identity.clone()),
        None,
        Some("artifact:chain-step4".into()),
        None,
    )
    .unwrap();
    let reassessed = replay_assessment(&reassess_snap).unwrap();
    assert_eq!(reassessed.status, ReplayStatus::Replayed);
    assert_eq!(reassessed.prior_explanation_valid, Some(false));
    assert!(reassessed.offline);
    assert!(!reassessed.reread_production_state);
    assert!(!reassessed.model_or_network_requests);
}
