//! DEC-012: typed AssessmentEnvelope + unknown/conflict semantics.
use awr_core::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn envelope_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/assessment/envelope")
}

fn load(name: &str) -> Value {
    let path = envelope_dir().join(name);
    serde_json::from_slice(
        &fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("json {}: {e}", path.display()))
}

fn base_identity() -> AssessmentIdentity {
    AssessmentIdentity {
        project_id: Some("01M1YJBR5PW6QXYGABADJVAJPC".into()),
        work_key: Some("AWR-DEC-012".into()),
        work_id: Some("01M2YQ3B5MWHZDHXE243CZNF7N".into()),
        branch_id: Some("main".into()),
        contract_fingerprint: Some("contract-a".into()),
        policy_id: ASSESSMENT_POLICY_ID.into(),
        policy_version: ASSESSMENT_POLICY_VERSION,
        policy_hash: Some("pol-v1".into()),
        input_summary: None,
        as_of: None,
        verified_main_sha: Some("8fa6679981c823842282a0a4b5b79b9c7165b2a4".into()),
        fact_snapshot_hash: None,
    }
}

fn snapshot_with_missing_and_conflict() -> FactSnapshot {
    build_fact_snapshot(FactSnapshotInput {
        identity: SnapshotIdentity {
            project_id: Some("01M1YJBR5PW6QXYGABADJVAJPC".into()),
            work_key: Some("AWR-DEC-012".into()),
            work_id: Some("01M2YQ3B5MWHZDHXE243CZNF7N".into()),
            branch_id: Some("main".into()),
            work_contract_hash: Some("contract-a".into()),
        },
        as_of: 100,
        policy_id: Some(ASSESSMENT_POLICY_ID.into()),
        policy_version: Some(ASSESSMENT_POLICY_VERSION),
        policy_hash: Some("pol-v1".into()),
        source_refs: vec!["source:a".into()],
        runtime_observation_refs: vec![],
        entity_versions: vec![],
        signals: vec![
            FactSignal::missing("workspace.changed_lines", SignalBasis::LocallyObserved),
            FactSignal {
                field: "work.owner".into(),
                value: Some(json!(["a", "b"])),
                unit: None,
                state: SignalState::Conflicting,
                basis: SignalBasis::SourceDeclared,
                classification: SignalClassification::Fact,
                origin_ref: None,
                observed_at: Some(100),
                scope: None,
                invalidation: None,
            },
            unsupported_change_risk_signal(),
        ],
        required_fields: vec![
            "workspace.changed_lines".into(),
            "work.owner".into(),
            "workspace.change_risk".into(),
        ],
        workspace_facts: Some(WorkspaceFacts::without_diff(
            WorkspaceFactSupply::HostSupplied,
            Some(100),
        )),
        read_scope: "assessment_read_only".into(),
        included: vec!["workspace".into()],
        limits: FactSnapshotLimitsInput::default(),
        candidates_seen: 3,
        scan_ops: 1,
    })
    .unwrap()
}

#[test]
fn compose_preserves_sub_assessment_conclusions_and_support_vocab() {
    let fixture = load("support-vocab.json");
    let mut assessments = vec![];
    for item in fixture["assessments"].as_array().unwrap() {
        let support = match item["support"].as_str().unwrap() {
            "supported" => AssessmentSupport::Supported,
            "unknown" => AssessmentSupport::Unknown,
            "conflicting" => AssessmentSupport::Conflicting,
            "unsupported" => AssessmentSupport::Unsupported,
            other => panic!("bad support {other}"),
        };
        assessments.push(AssessmentItem {
            id: item["id"].as_str().unwrap().into(),
            support,
            conclusion: item.get("conclusion").cloned().filter(|v| !v.is_null()),
            reason_codes: item["reason_codes"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(|v| v.as_str().unwrap().into())
                .collect(),
            basis_refs: item["basis_refs"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(|v| v.as_str().unwrap().into())
                .collect(),
            layer: None,
            heuristic_score: item.get("heuristic_score").and_then(|v| v.as_i64()),
            hard_reject: item["hard_reject"].as_bool().unwrap_or(false),
        });
    }
    let env = compose_assessment_envelope(AssessmentComposeInput {
        identity: base_identity(),
        as_of: 100,
        policy: AssessmentPolicy::default(),
        fact_snapshot: None,
        management: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        layers: BTreeMap::new(),
        assessments,
        advisory_actions: vec![],
        evidence_quality: None,
        evidence_applicability: Some("fixture".into()),
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec![],
        candidates_seen: 4,
        scan_ops: Some(1),
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })
    .unwrap();

    assert_eq!(env.schema_id, ASSESSMENT_ENVELOPE_SCHEMA_ID);
    let by_id: BTreeMap<_, _> = env.assessments.iter().map(|a| (a.id.clone(), a)).collect();
    assert_eq!(by_id["ready"].support, AssessmentSupport::Supported);
    assert_eq!(by_id["ready"].conclusion, Some(json!(true)));
    assert_eq!(by_id["diff"].support, AssessmentSupport::Unknown);
    assert!(by_id["diff"].conclusion.is_none());
    assert_eq!(by_id["owner"].support, AssessmentSupport::Conflicting);
    assert_eq!(
        by_id["model_success"].support,
        AssessmentSupport::Unsupported
    );
    // unknown was NOT coerced to false / low-risk
    assert_ne!(by_id["diff"].conclusion, Some(json!(false)));
    assert_ne!(by_id["diff"].conclusion, Some(json!("low_risk")));
    for layer in AssessmentLayerId::ALL {
        assert_eq!(
            env.layers[layer.as_str()].status,
            LayerStatus::NotEvaluated,
            "{}",
            layer.as_str()
        );
    }
}

#[test]
fn unknown_must_not_become_false_or_low_risk() {
    let bad_false = AssessmentItem {
        id: "diff".into(),
        support: AssessmentSupport::Unknown,
        conclusion: Some(json!(false)),
        reason_codes: vec![],
        basis_refs: vec![],
        layer: None,
        heuristic_score: None,
        hard_reject: false,
    };
    let err = compose_assessment_envelope(AssessmentComposeInput {
        identity: base_identity(),
        as_of: 1,
        policy: AssessmentPolicy::default(),
        fact_snapshot: None,
        management: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        layers: BTreeMap::new(),
        assessments: vec![bad_false],
        advisory_actions: vec![],
        evidence_quality: None,
        evidence_applicability: None,
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec![],
        candidates_seen: 1,
        scan_ops: None,
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("must not coerce conclusion to false")
    );

    let bad_low = AssessmentItem {
        id: "risk".into(),
        support: AssessmentSupport::Unknown,
        conclusion: Some(json!("low_risk")),
        reason_codes: vec![],
        basis_refs: vec![],
        layer: None,
        heuristic_score: None,
        hard_reject: false,
    };
    let err = compose_assessment_envelope(AssessmentComposeInput {
        identity: base_identity(),
        as_of: 1,
        policy: AssessmentPolicy::default(),
        fact_snapshot: None,
        management: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        layers: BTreeMap::new(),
        assessments: vec![bad_low],
        advisory_actions: vec![],
        evidence_quality: None,
        evidence_applicability: None,
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec![],
        candidates_seen: 1,
        scan_ops: None,
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })
    .unwrap_err();
    assert!(err.to_string().contains("low-risk"));
}

#[test]
fn evidence_quality_separates_coverage_gaps_conflicts_applicability_no_confidence() {
    let snap = snapshot_with_missing_and_conflict();
    let env = evaluate_assessment(&snap, AssessmentPolicy::default(), 100, None, None).unwrap();
    let eq = &env.evidence_quality;
    assert!(eq.missing.iter().any(|f| f == "workspace.changed_lines"));
    assert!(eq.conflicting.iter().any(|f| f == "work.owner"));
    assert!(eq.unsupported.iter().any(|f| f == "workspace.change_risk"));
    assert_eq!(
        eq.coverage_note,
        "coverage is a field count ratio, not probability"
    );
    assert_eq!(eq.applicability.as_deref(), Some("assessment_read_only"));
    // serialized evidence must not invent confidence/probability keys
    let encoded = serde_json::to_value(eq).unwrap();
    let obj = encoded.as_object().unwrap();
    for forbidden in [
        "confidence",
        "probability",
        "calibrated_probability",
        "p_success",
        "success_rate",
    ] {
        assert!(!obj.contains_key(forbidden), "unexpected {forbidden}");
    }
    let conf_err = compose_assessment_envelope(AssessmentComposeInput {
        identity: base_identity(),
        as_of: 1,
        policy: AssessmentPolicy::default(),
        fact_snapshot: None,
        management: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        layers: BTreeMap::new(),
        assessments: vec![AssessmentItem {
            id: "bad_prob".into(),
            support: AssessmentSupport::Supported,
            conclusion: Some(json!({"probability": 0.9})),
            reason_codes: vec![],
            basis_refs: vec![],
            layer: None,
            heuristic_score: Some(10),
            hard_reject: false,
        }],
        advisory_actions: vec![],
        evidence_quality: None,
        evidence_applicability: None,
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec![],
        candidates_seen: 1,
        scan_ops: None,
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })
    .unwrap_err();
    assert!(conf_err.to_string().contains("uncalibrated"));
}

#[test]
fn hard_rule_reject_cannot_be_offset_by_high_score() {
    let fixture = load("hard-rule-vs-score.json");
    let hard = AssessmentItem {
        id: "guard".into(),
        support: AssessmentSupport::Supported,
        conclusion: Some(json!("blocked")),
        reason_codes: vec![fixture["hard_reason"].as_str().unwrap().into()],
        basis_refs: vec!["policy.hard".into()],
        layer: Some(AssessmentLayerId::ExecutionAdmission),
        heuristic_score: Some(-100),
        hard_reject: true,
    };
    let soft = AssessmentItem {
        id: "score".into(),
        support: AssessmentSupport::Supported,
        conclusion: Some(json!("looks_fine")),
        reason_codes: vec!["independent_work_units".into()],
        basis_refs: vec!["heuristic".into()],
        layer: None,
        heuristic_score: Some(fixture["soft_score"].as_i64().unwrap()),
        hard_reject: false,
    };
    let env = compose_assessment_envelope(AssessmentComposeInput {
        identity: base_identity(),
        as_of: 50,
        policy: AssessmentPolicy::default(),
        fact_snapshot: None,
        management: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        layers: BTreeMap::new(),
        assessments: vec![soft.clone(), hard.clone()],
        advisory_actions: vec![AdvisoryAction {
            code: "no_advisory".into(),
            when: Some("hard gate rejects".into()),
            query_refs: vec![],
            limits: vec!["scores_do_not_clear_hard_rejects".into()],
            reevaluation: None,
        }],
        evidence_quality: None,
        evidence_applicability: None,
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec![],
        candidates_seen: 2,
        scan_ops: Some(1),
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })
    .unwrap();
    assert_eq!(env.hard_gate, HardGateOutcome::Reject);
    assert!(!soft_scores_may_rank(env.hard_gate));
    // hard reject sorts before soft score regardless of score magnitude
    assert_eq!(env.assessments[0].id, "guard");
    assert_eq!(env.assessments[1].id, "score");
    assert_eq!(
        env.layers["execution_admission"].status,
        LayerStatus::Supported
    );
}

#[test]
fn hard_unknown_stays_unknown_not_pass() {
    let env = compose_assessment_envelope(AssessmentComposeInput {
        identity: base_identity(),
        as_of: 1,
        policy: AssessmentPolicy::default(),
        fact_snapshot: None,
        management: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        layers: BTreeMap::new(),
        assessments: vec![AssessmentItem {
            id: "query_first".into(),
            support: AssessmentSupport::Unknown,
            conclusion: None,
            reason_codes: vec!["execution_result_requires_query".into()],
            basis_refs: vec!["runtime.execution".into()],
            layer: Some(AssessmentLayerId::DeliveryObservation),
            heuristic_score: Some(999),
            hard_reject: false,
        }],
        advisory_actions: vec![AdvisoryAction {
            code: "query_original_operation_result".into(),
            when: Some("execution outcome unknown".into()),
            query_refs: vec!["runtime.execution".into()],
            limits: vec!["do_not_retry_until_queried".into()],
            reevaluation: Some("after_original_result_known".into()),
        }],
        evidence_quality: None,
        evidence_applicability: None,
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec![],
        candidates_seen: 1,
        scan_ops: Some(1),
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })
    .unwrap();
    assert_eq!(env.hard_gate, HardGateOutcome::Unknown);
    assert!(!soft_scores_may_rank(env.hard_gate));
    assert_eq!(
        env.layers["delivery_observation"].status,
        LayerStatus::Unknown
    );
}

#[test]
fn reason_code_order_and_conflict_priority_are_deterministic() {
    let fixture = load("reason-order.json");
    let order: Vec<String> = fixture["expected_order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect();
    let mut assessments = vec![];
    for id in fixture["input_ids"].as_array().unwrap() {
        let id = id.as_str().unwrap();
        let (support, code, hard) = match id {
            "a_conflict" => (AssessmentSupport::Conflicting, "conflicting_signals", true),
            "b_query" => (
                AssessmentSupport::Unknown,
                "execution_result_requires_query",
                false,
            ),
            "c_units" => (
                AssessmentSupport::Supported,
                "independent_work_units",
                false,
            ),
            "d_claim" => (AssessmentSupport::Supported, "claim_conflict", true),
            other => panic!("{other}"),
        };
        assessments.push(AssessmentItem {
            id: id.into(),
            support,
            conclusion: None,
            reason_codes: vec![code.into()],
            basis_refs: vec![format!("ref:{id}")],
            layer: None,
            heuristic_score: None,
            hard_reject: hard,
        });
    }
    let env = compose_assessment_envelope(AssessmentComposeInput {
        identity: base_identity(),
        as_of: 7,
        policy: AssessmentPolicy::default(),
        fact_snapshot: None,
        management: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        layers: BTreeMap::new(),
        assessments,
        advisory_actions: vec![],
        evidence_quality: None,
        evidence_applicability: None,
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec![],
        candidates_seen: 4,
        scan_ops: Some(1),
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })
    .unwrap();
    let got: Vec<_> = env.assessments.iter().map(|a| a.id.clone()).collect();
    assert_eq!(got, order);
}

#[test]
fn resource_limits_truncate_and_do_not_claim_full_scan() {
    let fixture = load("resource-limits.json");
    let max = fixture["policy"]["max_assessments"].as_u64().unwrap() as usize;
    let mut assessments = vec![];
    for i in 0..fixture["candidates"].as_u64().unwrap() {
        assessments.push(AssessmentItem {
            id: format!("item_{i:03}"),
            support: AssessmentSupport::Supported,
            conclusion: Some(json!(i)),
            reason_codes: vec![],
            basis_refs: vec![format!("ref:{i}")],
            layer: None,
            heuristic_score: Some(i as i64),
            hard_reject: false,
        });
    }
    let mut policy = AssessmentPolicy::default();
    policy.max_assessments = max;
    let env = compose_assessment_envelope(AssessmentComposeInput {
        identity: base_identity(),
        as_of: 3,
        policy,
        fact_snapshot: None,
        management: None,
        management_observation: None,
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: None,
        management_next_action: None,
        action_rationale: None,
        layers: BTreeMap::new(),
        assessments,
        advisory_actions: vec![],
        evidence_quality: None,
        evidence_applicability: None,
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec![],
        candidates_seen: fixture["candidates"].as_u64().unwrap() as usize,
        scan_ops: Some(1),
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })
    .unwrap();
    assert_eq!(env.assessments.len(), max);
    assert!(env.limits.omitted_count > 0);
    assert_eq!(
        env.limits.truncation_reason.as_deref(),
        Some("assessments_exceeded")
    );
    // overrun must not report Pass pretending completeness
    assert_ne!(env.hard_gate, HardGateOutcome::Pass);
    assert_eq!(env.hard_gate, HardGateOutcome::Unknown);
}

#[test]
fn same_input_policy_as_of_yields_identical_canonical_hash() {
    let decision = decide_management(None, vec![], false);
    let guidance = ActionGuidance::new(
        "Management assessment has changed or has not been recorded",
        "management.record_required and contract_fingerprint",
        "Consume required context; record known observations once without inventing missing facts",
        "Scope, dependencies, execution outcome or waiting state changes",
    );
    let build = || {
        compose_assessment_envelope(AssessmentComposeInput {
            identity: base_identity(),
            as_of: 42,
            policy: AssessmentPolicy::default(),
            fact_snapshot: None,
            management: Some(decision.clone()),
            management_observation: None,
            management_observation_basis: None,
            management_admission_gaps: vec![],
            management_record_required: Some(true),
            management_next_action: None,
            action_rationale: Some(guidance.clone()),
            layers: BTreeMap::new(),
            assessments: vec![],
            advisory_actions: vec![AdvisoryAction {
                code: "record_management_observations".into(),
                when: Some("mode is undetermined because host facts are missing".into()),
                query_refs: vec!["management.decision.unknown_observations".into()],
                limits: vec!["do_not_invent_missing_facts".into()],
                reevaluation: None,
            }],
            evidence_quality: None,
            evidence_applicability: Some("management".into()),
            unsupported_fields: BTreeMap::from([(
                "layers.delivery_observation.changed_lines".into(),
                "unknown".into(),
            )]),
            legacy_source_views: vec!["awr_work_assess".into()],
            candidates_seen: 1,
            scan_ops: Some(1),
            time_budget_ms: None,
            read_scope: "assessment_read_only".into(),
        })
        .unwrap()
    };
    let a = build();
    let b = build();
    assert_eq!(a.assessment_hash, b.assessment_hash);
    assert_eq!(a.assessment_hash.len(), 64);
    assert_eq!(
        a.management.as_ref().unwrap().decision.mode,
        ManagementMode::Undetermined
    );
    assert_eq!(
        a.assessments
            .iter()
            .find(|x| x.id == "management_mode")
            .unwrap()
            .support,
        AssessmentSupport::Unknown
    );
    assert_eq!(
        a.unsupported_fields["layers.delivery_observation.changed_lines"],
        "unknown"
    );
    // replay compare via canonical JSON of semantic fields
    let canon = |e: &AssessmentEnvelope| {
        serde_json::to_vec(&json!({
            "assessments": e.assessments,
            "hard_gate": e.hard_gate,
            "layers": e.layers,
            "evidence_quality": e.evidence_quality,
            "advisory_actions": e.advisory_actions,
            "identity": e.identity,
        }))
        .unwrap()
    };
    assert_eq!(canon(&a), canon(&b));
}

#[test]
fn management_reuse_maps_admission_and_completion_constants() {
    let mut obs = ManagementObservation {
        observed_at: 1,
        note: "bounded single outcome".into(),
        single_outcome: Some(true),
        bounded_scope: Some(true),
        single_executor: Some(true),
        no_deferred_wait: Some(true),
        independently_schedulable_units: Some(1),
        plan_valid: Some(true),
        outcome_known: Some(true),
        ..Default::default()
    };
    let lightweight = decide_management(Some(&obs), vec![], false);
    assert_eq!(lightweight.mode, ManagementMode::Lightweight);
    let env = compose_assessment_envelope(AssessmentComposeInput {
        identity: base_identity(),
        as_of: 1,
        policy: AssessmentPolicy::default(),
        fact_snapshot: None,
        management: Some(lightweight),
        management_observation: Some(json!({"note": "bounded"})),
        management_observation_basis: None,
        management_admission_gaps: vec![],
        management_record_required: Some(false),
        management_next_action: None,
        action_rationale: None,
        layers: BTreeMap::new(),
        assessments: vec![],
        advisory_actions: vec![AdvisoryAction {
            code: "continue_prepare_current_work".into(),
            when: None,
            query_refs: vec![],
            limits: vec![],
            reevaluation: None,
        }],
        evidence_quality: None,
        evidence_applicability: None,
        unsupported_fields: BTreeMap::new(),
        legacy_source_views: vec![],
        candidates_seen: 1,
        scan_ops: Some(1),
        time_budget_ms: None,
        read_scope: "assessment_read_only".into(),
    })
    .unwrap();
    let m = env.management.as_ref().unwrap();
    assert_eq!(
        m.decision.execution_admission,
        "not_granted_by_management_classification"
    );
    assert_eq!(m.decision.completion_policy, "unchanged_source_policy");
    assert_eq!(
        env.layers["execution_admission"].summary.as_deref(),
        Some("not_granted_by_management_classification")
    );
    assert_eq!(
        env.layers["completion_validity"].summary.as_deref(),
        Some("unchanged_source_policy")
    );
    // silence unused mut warning path for obs fields we might tweak
    obs.note.push('.');
    let _ = obs;
}

#[test]
fn evaluate_assessment_requires_matching_as_of() {
    let snap = snapshot_with_missing_and_conflict();
    let err = evaluate_assessment(&snap, AssessmentPolicy::default(), 999, None, None).unwrap_err();
    assert!(err.to_string().contains("as_of must match"));
}
