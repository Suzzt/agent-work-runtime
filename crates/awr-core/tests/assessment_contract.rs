//! DEC-010: fixtures stay aligned with tip management/action constants.
use awr_core::*;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn contracts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/assessment/contracts")
}

fn load(name: &str) -> Value {
    let path = contracts_dir().join(name);
    serde_json::from_slice(
        &fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("json {}: {e}", path.display()))
}

fn small() -> ManagementObservation {
    ManagementObservation {
        observed_at: 1,
        note: "One bounded result, one owner, no deferred work".into(),
        single_outcome: Some(true),
        bounded_scope: Some(true),
        single_executor: Some(true),
        no_deferred_wait: Some(true),
        independently_schedulable_units: Some(1),
        plan_valid: Some(true),
        outcome_known: Some(true),
        ..Default::default()
    }
}

#[test]
fn reason_code_fixture_matches_tip_decide_management() {
    let reasons = load("reason-codes.json");
    let schema = load("schema.json");
    let unknown_fields: Vec<String> = reasons["unknown_observation_fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect();
    assert_eq!(unknown_fields.len(), 7);

    let unknown = decide_management(None, vec![], false);
    assert_eq!(unknown.mode, ManagementMode::Undetermined);
    assert_eq!(unknown.unknown_observations, unknown_fields);
    assert_eq!(
        unknown.completion_policy,
        schema["management_completion_policy"].as_str().unwrap()
    );
    assert_eq!(
        unknown.execution_admission,
        schema["management_execution_admission"].as_str().unwrap()
    );
    assert!(
        unknown
            .required_actions
            .iter()
            .any(|a| a == "assess_unknown_management_facts")
    );

    let triggers = reasons["host_assertion_triggers"].as_object().unwrap();
    for (field, code) in triggers {
        let mut o = small();
        match field.as_str() {
            "single_outcome" => o.single_outcome = Some(false),
            "bounded_scope" => o.bounded_scope = Some(false),
            "single_executor" => o.single_executor = Some(false),
            "no_deferred_wait" => o.no_deferred_wait = Some(false),
            "plan_valid" => o.plan_valid = Some(false),
            "outcome_known" => o.outcome_known = Some(false),
            other => panic!("unexpected trigger field {other}"),
        }
        let d = decide_management(Some(&o), vec![], false);
        assert_eq!(d.mode, ManagementMode::Continuous);
        assert!(
            d.reasons.iter().any(|r| r.code == code.as_str().unwrap()
                && r.basis == "host_assertion"
                && r.reference == *field),
            "missing reason for {field}"
        );
    }

    let mut units = small();
    units.independently_schedulable_units = Some(2);
    let d = decide_management(Some(&units), vec![], false);
    assert!(d.reasons.iter().any(|r| r.code == "independent_work_units"));

    let mut elapsed = small();
    elapsed.active_elapsed_ms = Some(30 * 60 * 1000);
    let d = decide_management(Some(&elapsed), vec![], false);
    assert_eq!(d.mode, ManagementMode::Lightweight);
    assert!(
        d.reevaluation_signals
            .iter()
            .any(|s| s == "active_elapsed_at_least_30_minutes")
    );
    assert!(
        d.required_actions
            .iter()
            .any(|a| a == "reevaluate_scope_plan_and_recovery_without_automatic_upgrade")
    );

    let continuous = decide_management(Some(&small()), vec![], true);
    assert_eq!(continuous.mode, ManagementMode::Continuous);
    assert!(
        continuous
            .reasons
            .iter()
            .any(|r| r.code == "continuous_management_retained")
    );
    for action in reasons["required_actions_continuous"].as_array().unwrap() {
        assert!(
            continuous
                .required_actions
                .iter()
                .any(|a| a == action.as_str().unwrap()),
            "missing continuous action {}",
            action
        );
    }
}

#[test]
fn envelope_example_and_guidance_budget_align_with_tip() {
    let schema = load("schema.json");
    let examples = load("examples.json");
    let consumers = load("consumers.json");
    let legacy = load("legacy-compat.json");
    let layers = load("layers.json");

    assert_eq!(ACTION_GUIDANCE_MAX_BYTES, 1024);
    assert_eq!(
        schema["action_guidance_max_bytes"].as_u64().unwrap(),
        ACTION_GUIDANCE_MAX_BYTES as u64
    );

    let env = &examples["minimal_undetermined"];
    assert_eq!(env["schema_id"], schema["schema_id"]);
    assert_eq!(
        env["management"]["decision"]["unknown_observations"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    assert_eq!(
        env["action_rationale"]["max_bytes"].as_u64().unwrap(),
        ACTION_GUIDANCE_MAX_BYTES as u64
    );
    for value in env["unsupported_fields"].as_object().unwrap().values() {
        assert_eq!(value, "unknown");
    }

    assert_eq!(consumers["count"], 2);
    assert_eq!(
        layers["first_batch_explanations"].as_array().unwrap().len(),
        2
    );
    assert_eq!(legacy["no_new_command_names"], true);
    assert!(
        legacy["do_not_occupy"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "decision show")
    );

    let guidance = ActionGuidance::new(
        env["action_rationale"]["when"].as_str().unwrap(),
        env["action_rationale"]["basis"].as_str().unwrap(),
        env["action_rationale"]["next_action"].as_str().unwrap(),
        env["action_rationale"]["recheck"].as_str().unwrap(),
    );
    let bytes = serde_json::to_vec(&guidance).unwrap();
    assert!(bytes.len() <= ACTION_GUIDANCE_MAX_BYTES);
}
