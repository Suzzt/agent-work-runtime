//! DEC-020: explanation chain over existing prepare/assess judgments.
use awr_core::*;
use awr_runtime::{
    CompletionExplanationInput, DeliveryExplanationInput, EXPLANATION_CHAIN_PROFILE,
    ExplanationAuthority, ExplanationChainInput, ExplanationChainResult, PreparedFactView,
    ProbeSupport, UnresolvedSideEffect, compose_explanation_chain, prior_explanation_still_valid,
};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

fn counterexamples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/assessment/counterexamples")
}

fn load_case(name: &str) -> Value {
    let path = counterexamples_dir().join(name);
    serde_json::from_slice(
        &fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("json {}: {e}", path.display()))
}

fn base_view() -> PreparedFactView {
    PreparedFactView {
        project_id: Some("01M1YJBR5PW6QXYGABADJVAJPC".into()),
        work_key: "AWR-DEC-020".into(),
        work_id: Some("01M2YQ3B5M1QH5N9R9TK3V14C3".into()),
        branch_id: Some("main".into()),
        work_revision: Some("1".into()),
        source_revision: Some("774".into()),
        work_contract_hash: Some("contract-a".into()),
        as_of: 100,
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

fn authority_from_view(view: &PreparedFactView) -> ExplanationAuthority {
    ExplanationAuthority {
        contract_hash: view.work_contract_hash.clone(),
        source_revision: view.source_revision.clone(),
        auth_fingerprint: Some("auth-ok".into()),
        stop_or_revoke: false,
    }
}

fn compose(
    view: PreparedFactView,
    delivery: DeliveryExplanationInput,
    completion: CompletionExplanationInput,
    authority: ExplanationAuthority,
    prior: Option<AssessmentIdentity>,
) -> ExplanationChainResult {
    let as_of = view.as_of;
    compose_explanation_chain(ExplanationChainInput {
        prepared: view,
        management_decision: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        delivery,
        completion,
        current_authority: authority,
        prior_identity: prior,
        policy: AssessmentPolicy::default(),
        as_of,
    })
    .expect("compose explanation chain")
}

/// Acceptance 1 (positive): five-layer conclusions preserved; envelope cites basis + unchecked.
#[test]
fn ac1_layers_preserve_conclusions_and_cite_basis_and_unchecked() {
    let view = base_view();
    let result = compose(
        view,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        authority_from_view(&base_view()),
        None,
    );
    assert_eq!(
        result.envelope.assessment_profile,
        EXPLANATION_CHAIN_PROFILE
    );
    assert_eq!(result.envelope.schema_id, ASSESSMENT_ENVELOPE_SCHEMA_ID);

    let layers = &result.envelope.layers;
    for id in AssessmentLayerId::ALL {
        assert!(
            layers.contains_key(id.as_str()),
            "missing layer {}",
            id.as_str()
        );
    }

    let readiness = &layers["work_readiness"];
    assert_eq!(readiness.status, LayerStatus::Supported);
    assert_eq!(readiness.summary.as_deref(), Some("ready"));
    assert!(readiness.basis_refs.iter().any(|r| r == "prepare.ready"));

    let admission = &layers["execution_admission"];
    assert_eq!(admission.status, LayerStatus::Supported);
    assert_eq!(
        admission.summary.as_deref(),
        Some("not_granted_by_management_classification")
    );

    let context = &layers["context_completeness"];
    assert_eq!(context.status, LayerStatus::Supported);
    assert_eq!(context.summary.as_deref(), Some("complete"));

    let completion = &layers["completion_validity"];
    assert_eq!(completion.status, LayerStatus::Supported);
    assert_eq!(
        completion.summary.as_deref(),
        Some("unchanged_source_policy")
    );

    // Delivery left unchecked when no delivery input — not_evaluated with unchecked note.
    let delivery = &layers["delivery_observation"];
    assert_eq!(delivery.status, LayerStatus::NotEvaluated);
    assert!(
        result.unchecked_by_layer["delivery_observation"]
            .iter()
            .any(|u| u == "delivery_observation")
            || delivery
                .summary
                .as_deref()
                .is_some_and(|s| s.contains("unchecked")),
        "delivery must expose unchecked items when not evaluated: {:?}",
        delivery
    );

    // Management continuous must not become admission.
    let admission_item = result
        .envelope
        .assessments
        .iter()
        .find(|a| a.id == "execution_admission")
        .unwrap();
    assert_eq!(
        admission_item.conclusion,
        Some(json!("not_granted_by_management_classification"))
    );
}

/// Acceptance 1 (negative): context complete must not force readiness ready.
#[test]
fn ac1_negative_context_complete_does_not_force_readiness() {
    let case = load_case("c01-deps-incomplete-context-complete.json");
    let mut view = base_view();
    view.ready = Some(false);
    view.context_complete = Some(true);
    view.diagnostics = vec![json!({"code": "unresolved_dependencies"})];
    let result = compose(
        view,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        ExplanationAuthority {
            contract_hash: Some("contract-a".into()),
            ..Default::default()
        },
        None,
    );
    let readiness = &result.envelope.layers["work_readiness"];
    assert!(
        matches!(
            readiness.status,
            LayerStatus::Unsupported | LayerStatus::Unknown
        ),
        "{:?}",
        readiness.status
    );
    let ready_item = result
        .envelope
        .assessments
        .iter()
        .find(|a| a.id == "work_readiness.ready")
        .unwrap();
    assert_eq!(ready_item.conclusion, Some(json!(false)));
    assert_eq!(
        result.envelope.layers["context_completeness"].status,
        LayerStatus::Supported
    );
    // Forbidden: conclude ready from context alone.
    assert_ne!(
        result.envelope.layers["work_readiness"].summary.as_deref(),
        Some("ready")
    );
    let expected = case["expect"]["work_readiness.ready"].as_bool().unwrap();
    assert!(!expected);
}

/// Known incomplete context is a false conclusion, not an unknown support.
#[test]
fn known_incomplete_context_stays_supported_false() {
    let mut view = base_view();
    view.context_complete = Some(false);
    view.context_issues = vec!["goal_missing".into()];
    let result = compose(
        view,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        authority_from_view(&base_view()),
        None,
    );
    let layer = &result.envelope.layers["context_completeness"];
    assert_eq!(layer.status, LayerStatus::Supported);
    assert_eq!(layer.summary.as_deref(), Some("incomplete"));
    let item = result
        .envelope
        .assessments
        .iter()
        .find(|item| item.id == "context_completeness")
        .unwrap();
    assert_eq!(item.support, AssessmentSupport::Supported);
    assert_eq!(item.conclusion, Some(json!(false)));
    assert!(item.reason_codes.iter().any(|code| code == "goal_missing"));
}

/// Acceptance 2 (positive): unresolved side effects → query original only.
#[test]
fn ac2_unresolved_side_effects_query_original_only() {
    let case = load_case("c03-pending-external-write-lost.json");
    let view = base_view();
    // High coverage / low risk / small change cues must not become re-run advice.
    let delivery = DeliveryExplanationInput {
        unresolved_side_effects: vec![UnresolvedSideEffect {
            original_op_ref: case["input"]["request_identity"].as_str().unwrap().into(),
            reason_code: "execution_result_requires_query".into(),
        }],
        soft_rerun_cues: vec![
            "high_coverage".into(),
            "low_risk".into(),
            "small_change".into(),
        ],
        exec_state_probe: Some(ProbeSupport::Unsupported),
        host_process_probe: Some(ProbeSupport::Unsupported),
        ..Default::default()
    };
    let result = compose(
        view,
        delivery,
        CompletionExplanationInput::default(),
        ExplanationAuthority::default(),
        None,
    );
    assert!(result.query_original_only);
    assert_eq!(result.envelope.advisory_actions.len(), 1);
    let advice = &result.envelope.advisory_actions[0];
    assert_eq!(advice.code, "query_original_operation_result");
    assert!(advice.query_refs.iter().any(|r| r == "req-42"));
    for forbid in [
        "do_not_rerun_for_high_coverage",
        "do_not_rerun_for_low_risk",
        "do_not_rerun_for_small_change",
    ] {
        assert!(
            advice.limits.iter().any(|l| l == forbid),
            "missing limit {forbid} in {:?}",
            advice.limits
        );
    }
    // No continue_prepare / refresh that implies safe retry.
    assert!(
        !result
            .envelope
            .advisory_actions
            .iter()
            .any(|a| a.code == "continue_prepare_current_work")
    );
    assert!(matches!(
        result.envelope.hard_gate,
        HardGateOutcome::Reject | HardGateOutcome::Unknown
    ));
}

/// Acceptance 2 (negative): soft cues alone must not invent a re-run advisory.
#[test]
fn ac2_negative_soft_cues_never_suggest_rerun() {
    let result = compose(
        base_view(),
        DeliveryExplanationInput {
            unresolved_side_effects: vec![UnresolvedSideEffect {
                original_op_ref: "op-9".into(),
                reason_code: "query_outcome_before_retry".into(),
            }],
            soft_rerun_cues: vec![
                "coverage_high".into(),
                "risk_low".into(),
                "small_diff".into(),
            ],
            ..Default::default()
        },
        CompletionExplanationInput::default(),
        ExplanationAuthority::default(),
        None,
    );
    for action in &result.envelope.advisory_actions {
        assert_eq!(action.code, "query_original_operation_result");
        let blob = format!(
            "{} {} {}",
            action.code,
            action.when.clone().unwrap_or_default(),
            action.reevaluation.clone().unwrap_or_default()
        )
        .to_ascii_lowercase();
        assert!(
            !blob.contains("rerun") && !blob.contains("retry_now"),
            "advisory must not encode re-run: {blob}"
        );
    }
}

/// Acceptance 3 (positive): source/auth change invalidates prior explanation.
#[test]
fn ac3_source_or_auth_change_invalidates_prior_explanation() {
    let case = load_case("c07-source-or-contract-changed.json");
    let prior = AssessmentIdentity {
        project_id: Some("01M1YJBR5PW6QXYGABADJVAJPC".into()),
        work_key: Some("AWR-DEC-020".into()),
        work_id: None,
        branch_id: Some("main".into()),
        contract_fingerprint: Some(
            case["input"]["envelope_contract_hash"]
                .as_str()
                .unwrap()
                .into(),
        ),
        policy_id: ASSESSMENT_POLICY_ID.into(),
        policy_version: 1,
        policy_hash: None,
        input_summary: Some("auth:auth-old".into()),
        as_of: Some(50),
        verified_main_sha: None,
        fact_snapshot_hash: Some("snap-old".into()),
    };
    let mut view = base_view();
    view.work_contract_hash = Some(
        case["input"]["current_contract_hash"]
            .as_str()
            .unwrap()
            .into(),
    );
    let authority = ExplanationAuthority {
        contract_hash: view.work_contract_hash.clone(),
        source_revision: Some("775".into()),
        auth_fingerprint: Some("auth-new".into()),
        stop_or_revoke: false,
    };
    assert!(!prior_explanation_still_valid(&prior, &authority));
    let result = compose(
        view,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        authority,
        Some(prior),
    );
    assert_eq!(result.prior_explanation_valid, Some(false));
    assert!(
        result
            .envelope
            .advisory_actions
            .iter()
            .any(|a| a.code == "refresh_sources"),
        "expected refresh_sources, got {:?}",
        result
            .envelope
            .advisory_actions
            .iter()
            .map(|a| &a.code)
            .collect::<Vec<_>>()
    );
}

/// Source revision and auth fingerprint are both retained. Changing only one invalidates.
#[test]
fn source_or_auth_alone_invalidates_emitted_identity() {
    let view = base_view();
    let authority = authority_from_view(&view);
    let first = compose(
        view.clone(),
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        authority.clone(),
        None,
    );
    let summary = first.envelope.identity.input_summary.as_deref().unwrap();
    assert!(
        summary.contains("source_revision=774") && summary.contains("auth=auth-ok"),
        "{summary}"
    );
    assert_eq!(
        first.envelope.assessment_hash,
        canonical_assessment_hash(&first.envelope).expect("canonical hash")
    );

    let mut source_changed = authority.clone();
    source_changed.source_revision = Some("775".into());
    let by_source = compose(
        view.clone(),
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        source_changed,
        Some(first.envelope.identity.clone()),
    );
    assert_eq!(by_source.prior_explanation_valid, Some(false));
    assert!(
        by_source
            .envelope
            .advisory_actions
            .iter()
            .any(|action| action.code == "refresh_sources")
    );

    let mut auth_changed = authority.clone();
    auth_changed.auth_fingerprint = Some("auth-new".into());
    let by_auth = compose(
        view.clone(),
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        auth_changed,
        Some(first.envelope.identity.clone()),
    );
    assert_eq!(by_auth.prior_explanation_valid, Some(false));

    let same = compose(
        view,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        authority,
        Some(first.envelope.identity),
    );
    assert_eq!(same.prior_explanation_valid, Some(true));
}

/// Acceptance 3: stop/revoke rejects cached advice; soft scores cannot clear.
#[test]
fn ac3_stop_or_revoke_rejects_cached_advice() {
    let case = load_case("c08-stop-or-revoke.json");
    let prior = AssessmentIdentity {
        project_id: None,
        work_key: Some("AWR-DEC-020".into()),
        work_id: None,
        branch_id: None,
        contract_fingerprint: Some("contract-a".into()),
        policy_id: ASSESSMENT_POLICY_ID.into(),
        policy_version: 1,
        policy_hash: None,
        input_summary: Some("auth:auth-ok".into()),
        as_of: Some(1),
        verified_main_sha: None,
        fact_snapshot_hash: None,
    };
    let result = compose(
        base_view(),
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        ExplanationAuthority {
            contract_hash: Some("contract-a".into()),
            source_revision: None,
            auth_fingerprint: Some("auth-ok".into()),
            stop_or_revoke: case["input"]["stop_or_revoke"].as_bool().unwrap(),
        },
        Some(prior),
    );
    assert_eq!(result.prior_explanation_valid, Some(false));
    assert_eq!(result.envelope.hard_gate, HardGateOutcome::Reject);
    assert!(
        result
            .envelope
            .advisory_actions
            .iter()
            .all(|a| a.code != "continue_prepare_current_work")
    );
    assert!(
        result
            .envelope
            .advisory_actions
            .iter()
            .any(|a| a.code == "no_advisory")
    );
}

/// Acceptance 3 (negative): unsupported probes return unknown and never claim stopped.
#[test]
fn ac3_unsupported_probes_return_unknown_without_claiming_stopped() {
    let result = compose(
        base_view(),
        DeliveryExplanationInput {
            historically_prepared: true,
            ack_present: Some(false),
            exec_state_probe: Some(ProbeSupport::Unsupported),
            host_process_probe: Some(ProbeSupport::Unsupported),
            ..Default::default()
        },
        CompletionExplanationInput::default(),
        ExplanationAuthority::default(),
        None,
    );
    let delivery = &result.envelope.layers["delivery_observation"];
    assert!(matches!(
        delivery.status,
        LayerStatus::Unknown | LayerStatus::NotEvaluated
    ));
    let probe_items: Vec<_> = result
        .envelope
        .assessments
        .iter()
        .filter(|a| a.id.contains("probe"))
        .collect();
    assert!(!probe_items.is_empty());
    for item in probe_items {
        assert_eq!(item.support, AssessmentSupport::Unknown);
        if let Some(Value::String(s)) = &item.conclusion {
            let lower = s.to_ascii_lowercase();
            assert_ne!(s, "stopped");
            assert!(!lower.contains("stopped"));
            assert!(!lower.contains("killed"));
            assert!(!lower.contains("terminated"));
            assert_eq!(s, "unknown");
        }
    }
    // C15: prepared without ack → not_verified / unknown.
    let ack = result
        .envelope
        .assessments
        .iter()
        .find(|a| a.id == "delivery.ack")
        .unwrap();
    assert_eq!(ack.support, AssessmentSupport::Unknown);
    assert_eq!(ack.conclusion, Some(json!("not_verified")));
}

/// Positive boundary P01: readiness+context supported; admission still not granted.
#[test]
fn positive_p01_admission_boundary_preserved() {
    let case = load_case("p01-ready-context-admission-boundary.json");
    let result = compose(
        base_view(),
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        ExplanationAuthority {
            contract_hash: Some("contract-a".into()),
            ..Default::default()
        },
        None,
    );
    assert_eq!(
        result.envelope.layers["work_readiness"].status,
        LayerStatus::Supported
    );
    assert_eq!(
        result.envelope.layers["context_completeness"].status,
        LayerStatus::Supported
    );
    assert_eq!(
        result.envelope.layers["execution_admission"]
            .summary
            .as_deref(),
        case["expect"]["execution_admission"].as_str()
    );
}

/// Same inputs → identical assessment_hash (replay / incremental consistency).
#[test]
fn same_inputs_yield_identical_explanation_hash() {
    let view = base_view();
    let delivery = DeliveryExplanationInput {
        waiting_user: true,
        wait_refs: vec!["wait-1".into()],
        ..Default::default()
    };
    let a = compose(
        view.clone(),
        delivery.clone(),
        CompletionExplanationInput::default(),
        authority_from_view(&view),
        None,
    );
    let b = compose(
        view,
        delivery,
        CompletionExplanationInput::default(),
        authority_from_view(&base_view()),
        None,
    );
    assert_eq!(a.envelope.assessment_hash, b.envelope.assessment_hash);
}

/// Management mode conclusions stay bound; chain does not invent a second adjudicator.
#[test]
fn reuses_management_decision_without_second_adjudicator() {
    let mut view = base_view();
    // Inject continuous mode + unresolved deps reason already decided by tip assess.
    view.management = Some(json!({
        "contract_fingerprint": "contract-a",
        "decision": {
            "version": 1,
            "mode": "continuous",
            "reasons": [{
                "code": "unresolved_dependencies",
                "basis": "source_projection",
                "reference": "AWR-DEC-020"
            }],
            "unknown_observations": [],
            "reevaluation_signals": [],
            "required_actions": ["query_unknown_results_before_any_retry"],
            "optional_maintenance": [],
            "completion_policy": "unchanged_source_policy",
            "execution_admission": "not_granted_by_management_classification"
        },
        "record_required": false,
        "admission_gaps": [],
        "next_action": "follow_required_actions_and_existing_workflow"
    }));
    view.ready = Some(false);
    view.diagnostics = vec![json!({"code": "unresolved_dependencies"})];
    let result = compose(
        view,
        DeliveryExplanationInput::default(),
        CompletionExplanationInput::default(),
        ExplanationAuthority {
            contract_hash: Some("contract-a".into()),
            ..Default::default()
        },
        None,
    );
    let mgmt = result.envelope.management.as_ref().unwrap();
    assert_eq!(mgmt.decision.mode, ManagementMode::Continuous);
    assert_eq!(
        mgmt.decision.execution_admission,
        "not_granted_by_management_classification"
    );
    // Continuous mode still does not grant admission via the explanation chain.
    assert_eq!(
        result.envelope.layers["execution_admission"]
            .summary
            .as_deref(),
        Some("not_granted_by_management_classification")
    );
}
