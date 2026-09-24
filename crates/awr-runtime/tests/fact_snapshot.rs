//! DEC-011 runtime mapping from one prepared read view (no Store re-query).
use awr_core::*;
use awr_runtime::{
    PreparedFactView, fact_snapshot_from_prepared_view, prepared_view_from_prepare_json,
};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

fn signals_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/assessment/signals")
}

fn load(name: &str) -> Value {
    let path = signals_dir().join(name);
    serde_json::from_slice(
        &fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("json {}: {e}", path.display()))
}

#[test]
fn stale_and_partial_prepared_views_label_quality() {
    let stale = load("stale-snapshot.json");
    let view: PreparedFactView = serde_json::from_value(stale["prepared_view"].clone()).unwrap();
    let snap = fact_snapshot_from_prepared_view(&view).unwrap();
    assert_eq!(
        snap.identity.project_id.as_deref(),
        view.project_id.as_deref()
    );
    assert_eq!(snap.identity.work_key.as_deref(), Some("AWR-DEC-011"));
    assert_eq!(snap.limits.scan_ops, 1, "must reuse one read-only query");
    let assessment = snap
        .signals
        .iter()
        .find(|s| s.field == "management.assessment")
        .unwrap();
    assert_eq!(assessment.state, SignalState::Stale);
    assert!(
        snap.coverage
            .stale
            .contains(&"management.assessment".into())
    );

    let partial = load("partial-results.json");
    let view: PreparedFactView = serde_json::from_value(partial["prepared_view"].clone()).unwrap();
    let snap = fact_snapshot_from_prepared_view(&view).unwrap();
    assert_eq!(snap.limits.scan_ops, 1);
    for field in partial["expect"]["missing_contains"].as_array().unwrap() {
        assert!(
            snap.coverage
                .missing
                .iter()
                .any(|f| f == field.as_str().unwrap())
                || snap
                    .signals
                    .iter()
                    .any(|s| s.field == field.as_str().unwrap()
                        && matches!(s.state, SignalState::Missing | SignalState::Unsupported)),
            "expected missing {}",
            field
        );
    }
    assert!(
        snap.coverage
            .host_asserted_only
            .contains(&"management.single_outcome".into())
    );
}

#[test]
fn missing_git_diff_on_prepared_view_does_not_zero_fill() {
    let fixture = load("missing-git-diff.json");
    let ws: WorkspaceFacts = serde_json::from_value(fixture["workspace_facts"].clone()).unwrap();
    let view = PreparedFactView {
        project_id: Some("p1".into()),
        work_key: "AWR-DEC-011".into(),
        work_id: Some("w1".into()),
        branch_id: None,
        work_revision: Some("1".into()),
        source_revision: Some("s1".into()),
        work_contract_hash: Some("c1".into()),
        as_of: 50,
        ready: Some(true),
        diagnostics: vec![],
        active_claim_ids: vec![],
        management: None,
        context_complete: None,
        context_issues: vec![],
        source_refs: vec![],
        runtime_observation_refs: vec![],
        host_observation: None,
        workspace_facts: Some(ws),
        limits: None,
    };
    let snap = fact_snapshot_from_prepared_view(&view).unwrap();
    let lines = snap
        .signals
        .iter()
        .find(|s| s.field == "workspace.changed_lines")
        .unwrap();
    assert_eq!(lines.state, SignalState::Missing);
    assert_ne!(lines.value, Some(json!(0)));
    assert!(lines.value.is_none());
    let risk = snap
        .signals
        .iter()
        .find(|s| s.field == "workspace.change_risk")
        .unwrap();
    assert_eq!(risk.state, SignalState::Unsupported);
    if let Some(ws) = &snap.workspace_facts {
        assert!(ws.changed_lines.is_none());
        assert_ne!(ws.change_risk.as_deref(), Some("low"));
    }
}

#[test]
fn prepare_json_mapping_reuses_envelope_without_extra_scan() {
    let prepare = json!({
        "version": 1,
        "stage": "prepared",
        "work": {
            "id": "01M2YQ3B5M9PB6AJ16PW043BT3",
            "external_key": "AWR-DEC-011",
            "status": "in_progress",
            "revision": 2,
            "source_ref": {"source_revision": "src-9"}
        },
        "ready": true,
        "diagnostics": [{"code": "ok"}],
        "active_claims": [{"id": "claim-1"}],
        "management": {
            "contract_fingerprint": "fp-9",
            "decision": {"mode": "undetermined"},
            "observation": {
                "observed_at": 10,
                "note": "host note for prepare mapping fixture",
                "single_outcome": true,
                "bounded_scope": true,
                "single_executor": true,
                "no_deferred_wait": true,
                "independently_schedulable_units": 1,
                "plan_valid": true,
                "outcome_known": true
            }
        },
        "context": {"completeness": {"complete": true, "branch_id": null, "issues": []}}
    });
    let view = prepared_view_from_prepare_json(
        &prepare,
        Some("01M1YJBR5PW6QXYGABADJVAJPC".into()),
        99,
        None,
    )
    .unwrap();
    let snap = fact_snapshot_from_prepared_view(&view).unwrap();
    assert_eq!(snap.as_of, 99);
    assert_eq!(snap.limits.scan_ops, 1);
    assert!(snap.scope.included.iter().any(|s| s == "work_readiness"));
    assert!(snap.scope.included.iter().any(|s| s == "management"));
    assert_eq!(snap.identity.work_contract_hash.as_deref(), Some("fp-9"));
    // No workspace facts → changed_lines missing, not zero.
    assert!(
        snap.signals
            .iter()
            .any(|s| s.field == "workspace.changed_lines" && s.state == SignalState::Missing)
    );
}

#[test]
fn conflicting_entity_versions_in_view_are_rejected() {
    let view = PreparedFactView {
        project_id: Some("p1".into()),
        work_key: "AWR-DEC-011".into(),
        work_id: Some("w1".into()),
        branch_id: None,
        work_revision: Some("1".into()),
        source_revision: Some("s1".into()),
        work_contract_hash: Some("c1".into()),
        as_of: 1,
        ready: Some(true),
        diagnostics: vec![],
        active_claim_ids: vec![],
        management: Some(json!({"contract_fingerprint": "c1"})),
        context_complete: None,
        context_issues: vec![],
        source_refs: vec![],
        runtime_observation_refs: vec![],
        host_observation: None,
        workspace_facts: None,
        limits: None,
    };
    // First build succeeds.
    fact_snapshot_from_prepared_view(&view).unwrap();
    // Inject conflict via core builder path.
    let err = build_fact_snapshot(FactSnapshotInput {
        identity: SnapshotIdentity {
            project_id: view.project_id.clone(),
            work_key: Some(view.work_key.clone()),
            work_id: view.work_id.clone(),
            branch_id: None,
            work_contract_hash: view.work_contract_hash.clone(),
        },
        as_of: 1,
        policy_id: None,
        policy_version: None,
        policy_hash: None,
        source_refs: vec![],
        runtime_observation_refs: vec![],
        entity_versions: vec![
            EntityVersionRef {
                entity_kind: "work".into(),
                entity_id: "w1".into(),
                version: "1".into(),
            },
            EntityVersionRef {
                entity_kind: "work".into(),
                entity_id: "w1".into(),
                version: "2".into(),
            },
        ],
        signals: vec![],
        required_fields: vec![],
        workspace_facts: None,
        read_scope: "assessment_read_only".into(),
        included: vec![],
        limits: FactSnapshotLimitsInput::default(),
        candidates_seen: 0,
        scan_ops: 1,
    })
    .unwrap_err();
    assert!(err.to_string().contains("entity version conflict"));
}
