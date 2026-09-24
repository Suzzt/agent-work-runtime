//! DEC-011: bounded FactSnapshot + source-quality labeling.
use awr_core::*;
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

fn base_identity() -> SnapshotIdentity {
    SnapshotIdentity {
        project_id: Some("01M1YJBR5PW6QXYGABADJVAJPC".into()),
        work_key: Some("AWR-DEC-011".into()),
        work_id: Some("01M2YQ3B5M9PB6AJ16PW043BT3".into()),
        branch_id: Some("main".into()),
        work_contract_hash: Some("contract-a".into()),
    }
}

#[test]
fn snapshot_carries_identity_versions_as_of_scope_and_truncation() {
    let snap = build_fact_snapshot(FactSnapshotInput {
        identity: base_identity(),
        as_of: 42,
        policy_id: Some("tip_management_v1".into()),
        policy_version: Some(1),
        policy_hash: Some("pol".into()),
        source_refs: vec!["source:a".into()],
        runtime_observation_refs: vec!["event:1".into()],
        entity_versions: vec![EntityVersionRef {
            entity_kind: "work".into(),
            entity_id: "01M2YQ3B5M9PB6AJ16PW043BT3".into(),
            version: "3".into(),
        }],
        signals: vec![FactSignal::known(
            "work.ready",
            json!(true),
            SignalBasis::RuntimeRecorded,
            Some("work_readiness".into()),
            Some(42),
        )],
        required_fields: vec!["work.ready".into()],
        workspace_facts: None,
        read_scope: "assessment_read_only".into(),
        included: vec!["work_readiness".into()],
        limits: FactSnapshotLimitsInput::default(),
        candidates_seen: 1,
        scan_ops: 1,
    })
    .unwrap();
    assert_eq!(snap.schema_id, FACT_SNAPSHOT_SCHEMA_ID);
    assert_eq!(snap.as_of, 42);
    assert_eq!(snap.identity.work_key.as_deref(), Some("AWR-DEC-011"));
    assert_eq!(
        snap.identity.work_contract_hash.as_deref(),
        Some("contract-a")
    );
    assert!(!snap.scope.truncated);
    assert_eq!(snap.entity_versions.len(), 1);
    assert!(!snap.content_hash.is_empty());
    assert_eq!(snap.content_hash.len(), 64);
}

#[test]
fn same_snapshot_entity_version_conflict_is_rejected() {
    let fixture = load("version-conflict.json");
    let versions: Vec<EntityVersionRef> =
        serde_json::from_value(fixture["entity_versions"].clone()).unwrap();
    let err = reject_entity_version_conflicts(&versions).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains(fixture["expect"]["error_contains"].as_str().unwrap()),
        "{msg}"
    );
    let build_err = build_fact_snapshot(FactSnapshotInput {
        identity: base_identity(),
        as_of: 1,
        policy_id: None,
        policy_version: None,
        policy_hash: None,
        source_refs: vec![],
        runtime_observation_refs: vec![],
        entity_versions: versions,
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
    assert!(build_err.to_string().contains("entity version conflict"));
}

#[test]
fn duplicate_identical_entity_versions_are_accepted() {
    let fixture = load("duplicate-sources.json");
    let versions: Vec<EntityVersionRef> =
        serde_json::from_value(fixture["entity_versions"].clone()).unwrap();
    reject_entity_version_conflicts(&versions).unwrap();
}

#[test]
fn missing_stale_conflict_unsupported_host_asserted_and_verified_are_distinct() {
    let fixture = load("verified-host-asserted.json");
    let mut signals = vec![
        FactSignal {
            field: "workspace.changed_lines".into(),
            value: None,
            unit: Some("lines".into()),
            state: SignalState::Missing,
            basis: SignalBasis::LocallyObserved,
            classification: SignalClassification::Observation,
            origin_ref: None,
            observed_at: None,
            scope: None,
            invalidation: None,
        },
        FactSignal {
            field: "management.assessment".into(),
            value: Some(json!("old-fp")),
            unit: None,
            state: SignalState::Stale,
            basis: SignalBasis::RuntimeRecorded,
            classification: SignalClassification::Fact,
            origin_ref: None,
            observed_at: Some(1),
            scope: None,
            invalidation: Some("work_contract_changed".into()),
        },
        FactSignal {
            field: "work.owner".into(),
            value: Some(json!(["a", "b"])),
            unit: None,
            state: SignalState::Conflicting,
            basis: SignalBasis::SourceDeclared,
            classification: SignalClassification::Fact,
            origin_ref: None,
            observed_at: Some(1),
            scope: None,
            invalidation: None,
        },
        FactSignal {
            field: "workspace.change_risk".into(),
            value: None,
            unit: None,
            state: SignalState::Unsupported,
            basis: SignalBasis::RuleDerived,
            classification: SignalClassification::Inference,
            origin_ref: None,
            observed_at: None,
            scope: None,
            invalidation: None,
        },
    ];
    for item in fixture["signals"].as_array().unwrap() {
        let basis = match item["basis"].as_str().unwrap() {
            "source_declared" => SignalBasis::SourceDeclared,
            "host_asserted" => SignalBasis::HostAsserted,
            "rule_derived" => SignalBasis::RuleDerived,
            other => panic!("unexpected basis {other}"),
        };
        let classification = match item["classification"].as_str().unwrap() {
            "fact" => SignalClassification::Fact,
            "observation" => SignalClassification::Observation,
            "inference" => SignalClassification::Inference,
            other => panic!("unexpected classification {other}"),
        };
        signals.push(FactSignal {
            field: item["field"].as_str().unwrap().into(),
            value: Some(item["value"].clone()),
            unit: None,
            state: SignalState::Known,
            basis,
            classification,
            origin_ref: None,
            observed_at: Some(1),
            scope: None,
            invalidation: None,
        });
    }
    let snap = build_fact_snapshot(FactSnapshotInput {
        identity: base_identity(),
        as_of: 1,
        policy_id: None,
        policy_version: None,
        policy_hash: None,
        source_refs: vec![],
        runtime_observation_refs: vec![],
        entity_versions: vec![],
        signals,
        required_fields: vec![
            "workspace.changed_lines".into(),
            "workspace.change_risk".into(),
            "work.status".into(),
        ],
        workspace_facts: None,
        read_scope: "assessment_read_only".into(),
        included: vec![],
        limits: FactSnapshotLimitsInput::default(),
        candidates_seen: 8,
        scan_ops: 1,
    })
    .unwrap();
    assert!(
        snap.coverage
            .missing
            .contains(&"workspace.changed_lines".into())
    );
    assert!(
        snap.coverage
            .stale
            .contains(&"management.assessment".into())
    );
    assert!(snap.coverage.conflicting.contains(&"work.owner".into()));
    assert!(
        snap.coverage
            .unsupported
            .contains(&"workspace.change_risk".into())
    );
    assert!(
        snap.coverage
            .host_asserted_only
            .contains(&"management.single_outcome".into())
    );
    let status = snap
        .signals
        .iter()
        .find(|s| s.field == "work.status")
        .unwrap();
    assert!(status.is_verified_fact());
    let host = snap
        .signals
        .iter()
        .find(|s| s.field == "management.single_outcome")
        .unwrap();
    assert!(!host.is_verified_fact());
    assert_eq!(host.classification, SignalClassification::Observation);
}

#[test]
fn missing_git_diff_never_emits_changed_lines_zero_or_low_risk() {
    let fixture = load("missing-git-diff.json");
    let ws: WorkspaceFacts = serde_json::from_value(fixture["workspace_facts"].clone()).unwrap();
    ws.validate().unwrap();
    assert!(ws.changed_lines.is_none());
    assert!(ws.change_risk.is_none());

    let snap = build_fact_snapshot(FactSnapshotInput {
        identity: base_identity(),
        as_of: 50,
        policy_id: None,
        policy_version: None,
        policy_hash: None,
        source_refs: vec![],
        runtime_observation_refs: vec![],
        entity_versions: vec![],
        signals: vec![missing_git_diff_signal(), unsupported_change_risk_signal()],
        required_fields: vec![
            "workspace.changed_lines".into(),
            "workspace.change_risk".into(),
        ],
        workspace_facts: Some(ws),
        read_scope: "assessment_read_only".into(),
        included: vec!["workspace_facts".into()],
        limits: FactSnapshotLimitsInput::default(),
        candidates_seen: 2,
        scan_ops: 1,
    })
    .unwrap();
    let lines = snap
        .signals
        .iter()
        .find(|s| s.field == "workspace.changed_lines")
        .unwrap();
    assert_eq!(lines.state, SignalState::Missing);
    assert!(lines.value.is_none());
    let risk = snap
        .signals
        .iter()
        .find(|s| s.field == "workspace.change_risk")
        .unwrap();
    assert_eq!(risk.state, SignalState::Unsupported);

    let mut bad = WorkspaceFacts::without_diff(WorkspaceFactSupply::HostSupplied, Some(1));
    bad.changed_lines = Some(0);
    assert!(bad.validate().is_err());
    bad.changed_lines = None;
    bad.change_risk = Some("low".into());
    assert!(bad.validate().is_err());
}

#[test]
fn over_limit_inputs_truncate_without_inventing_values() {
    let fixture = load("over-limit.json");
    let limits: FactSnapshotLimitsInput =
        serde_json::from_value(fixture["limits"].clone()).unwrap();
    let count = fixture["signal_count"].as_u64().unwrap() as usize;
    let signals: Vec<_> = (0..count)
        .map(|i| {
            FactSignal::known(
                &format!("field.{i}"),
                json!(i),
                SignalBasis::RuntimeRecorded,
                None,
                Some(1),
            )
        })
        .collect();
    let snap = build_fact_snapshot(FactSnapshotInput {
        identity: base_identity(),
        as_of: 1,
        policy_id: None,
        policy_version: None,
        policy_hash: None,
        source_refs: vec![],
        runtime_observation_refs: vec![],
        entity_versions: vec![],
        signals,
        required_fields: vec![],
        workspace_facts: None,
        read_scope: "assessment_read_only".into(),
        included: vec![],
        limits,
        candidates_seen: count,
        scan_ops: 1,
    })
    .unwrap();
    assert_eq!(snap.scope.truncated, fixture["expect"]["truncated"]);
    assert_eq!(
        snap.scope.truncation_reason.as_deref(),
        fixture["expect"]["truncation_reason"].as_str()
    );
    assert!(
        snap.limits.omitted_count
            >= fixture["expect"]["omitted_count_at_least"]
                .as_u64()
                .unwrap() as usize
    );
    assert!(snap.signals.len() <= snap.limits.max_candidates);
}

#[test]
fn host_asserted_cannot_be_labeled_verified_fact() {
    let err = build_fact_snapshot(FactSnapshotInput {
        identity: base_identity(),
        as_of: 1,
        policy_id: None,
        policy_version: None,
        policy_hash: None,
        source_refs: vec![],
        runtime_observation_refs: vec![],
        entity_versions: vec![],
        signals: vec![FactSignal {
            field: "management.single_outcome".into(),
            value: Some(json!(true)),
            unit: None,
            state: SignalState::Known,
            basis: SignalBasis::HostAsserted,
            classification: SignalClassification::Fact,
            origin_ref: None,
            observed_at: Some(1),
            scope: None,
            invalidation: None,
        }],
        required_fields: vec![],
        workspace_facts: None,
        read_scope: "assessment_read_only".into(),
        included: vec![],
        limits: FactSnapshotLimitsInput::default(),
        candidates_seen: 1,
        scan_ops: 1,
    })
    .unwrap_err();
    assert!(err.to_string().contains("observation"));
}

#[test]
fn raising_limits_above_ceilings_is_rejected() {
    let err = FactSnapshotLimitsInput {
        max_candidates: FACT_SNAPSHOT_MAX_CANDIDATES + 1,
        max_bytes: FACT_SNAPSHOT_MAX_BYTES,
        max_scan_ops: FACT_SNAPSHOT_MAX_SCAN_OPS,
    }
    .capped()
    .unwrap_err();
    assert!(err.to_string().contains("limits"));
}
