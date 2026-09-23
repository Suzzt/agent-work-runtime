//! Integration coverage for AWR-WS-024 named adapters + subtask parallelism.
use awr_core::{
    AdapterCapability, AdapterId, AdapterNegotiationRequest, ExternalExecutionReport,
    ExternalReportOrigin, ExternalReportPhase, Id, NegotiationDecision, SubtaskIdentity,
    SubtaskResourceBound, now_millis,
};
use awr_runtime::host_adapter::{
    AdapterActionOutcome, CodexCliAdapter, ExecutionHostAdapter, NativeExecutionHandle,
    ParallelScheduler, PauseGate, built_in_registry, refuse_coordination_as_process_control,
    rollup_refs,
};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn fixture(name: &str) -> Value {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../tests/fixtures/workstreams/named-agent-host");
    path.push(name);
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn fixture_matrix_matches_built_in_registry() {
    let doc = fixture("capability-matrix.json");
    let reg = built_in_registry();
    let adapters = doc["adapters"].as_array().unwrap();
    assert_eq!(adapters.len(), 3);
    for entry in adapters {
        let id = entry["adapter_id"].as_str().unwrap();
        let adapter = reg.get(id).expect("adapter present");
        let expected: BTreeSet<String> = entry["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let actual: BTreeSet<String> = adapter
            .matrix()
            .capabilities
            .iter()
            .map(|c| c.as_str().to_string())
            .collect();
        assert_eq!(actual, expected, "caps for {id}");
        assert_eq!(
            adapter.matrix().auto_startable,
            entry["auto_startable"].as_bool().unwrap()
        );
    }
    assert_eq!(reg.named_controlled().len(), 2);
}

#[test]
fn l0_report_path_and_two_adapters() {
    let reg = built_in_registry();
    let l0 = reg.get("manual_report").unwrap();
    let report = ExternalExecutionReport {
        version: 1,
        request_key: "req-1".into(),
        execution_id: Id::from(1u128),
        host_id: "fixture-host".into(),
        host_work_key: "ws024".into(),
        native_session: "generic:conv-1".into(),
        agent_id: "operator".into(),
        origin: ExternalReportOrigin::CallerReported,
        phase: ExternalReportPhase::Started,
        observed_at: now_millis().unwrap(),
        summary: "L0 manual start observation".into(),
        detail_references: vec!["host://fixture/log".into()],
    };
    assert!(matches!(
        l0.accept_l0_report(&report).unwrap(),
        AdapterActionOutcome::Supported { .. }
    ));

    let codex = reg.get("codex_cli").unwrap();
    let claude = reg.get("claude_code").unwrap();
    let handle = NativeExecutionHandle {
        adapter_id: AdapterId::new("codex_cli").unwrap(),
        execution_id: "exec-1".into(),
        native_session: "codex:1".into(),
        operation_key: "op-1".into(),
    };
    assert!(matches!(
        codex.start(&handle).unwrap(),
        AdapterActionOutcome::HumanContinuation {
            missing: AdapterCapability::Start,
            ..
        }
    ));
    assert!(matches!(
        claude
            .start(&NativeExecutionHandle {
                adapter_id: AdapterId::new("claude_code").unwrap(),
                execution_id: "exec-2".into(),
                native_session: "claude:1".into(),
                operation_key: "op-2".into(),
            })
            .unwrap(),
        AdapterActionOutcome::HumanContinuation {
            missing: AdapterCapability::Start,
            ..
        }
    ));
    // Status negotiation remains usable; native start/stop are not claimed.
    for id in ["codex_cli", "claude_code"] {
        let adapter = reg.get(id).unwrap();
        assert!(!adapter.matrix().auto_startable);
        let result = adapter.negotiate(&AdapterNegotiationRequest {
            adapter_id: AdapterId::new(id).unwrap(),
            required: BTreeSet::from([AdapterCapability::StatusRead]),
            optional: BTreeSet::new(),
        });
        assert_eq!(result.decision, NegotiationDecision::Usable);
        assert!(!adapter.matrix().supports(AdapterCapability::Start));
        assert!(
            !adapter
                .matrix()
                .supports(AdapterCapability::StopConfirmation)
        );
        assert!(
            !adapter
                .matrix()
                .supports(AdapterCapability::ReconnectResume)
        );
    }
}

#[test]
fn codex_in_memory_handle_is_not_verified_native_status() {
    let adapter = CodexCliAdapter::new();
    // Direct observed_status without L0 report must not advertise verified running.
    assert!(adapter.observed_status("never-started").is_none());
    // Accepting an L0 report is the attributable observation path.
    let report = ExternalExecutionReport {
        version: 1,
        request_key: "req-obs".into(),
        execution_id: Id::from(9u128),
        host_id: "fixture-host".into(),
        host_work_key: "ws024".into(),
        native_session: "codex:obs".into(),
        agent_id: "operator".into(),
        origin: ExternalReportOrigin::CallerReported,
        phase: ExternalReportPhase::Started,
        observed_at: now_millis().unwrap(),
        summary: "observed via L0".into(),
        detail_references: vec![],
    };
    adapter.accept_l0_report(&report).unwrap();
    let status = adapter
        .observed_status(&report.execution_id.to_string())
        .expect("status from L0");
    assert!(status.verified);
    assert_eq!(status.basis, "codex_cli_l0_external_report");
}

#[test]
fn parallel_fixture_plan_and_parent_isolation() {
    let doc = fixture("parallel-children.json");
    let mut sched = ParallelScheduler::new(
        doc["parent_session_id"].as_str().unwrap(),
        doc["concurrency_cap"].as_u64().unwrap() as usize,
    )
    .unwrap();
    for child in doc["children"].as_array().unwrap() {
        let bounds = child["resource_bounds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| SubtaskResourceBound {
                kind: b["kind"].as_str().unwrap().into(),
                key: b["key"].as_str().unwrap().into(),
                worktree_id: b["worktree_id"].as_str().unwrap_or("").into(),
            })
            .collect();
        let depends = child["depends_on"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        sched
            .register(SubtaskIdentity {
                work_id: child["work_id"].as_str().unwrap().into(),
                claim_id: child["claim_id"].as_str().unwrap().into(),
                parent_session_id: doc["parent_session_id"].as_str().unwrap().into(),
                child_session_id: child["child_session_id"].as_str().unwrap().into(),
                agent_label: child["agent_label"].as_str().unwrap().into(),
                resource_bounds: bounds,
                depends_on: depends,
            })
            .unwrap();
    }
    let plan = sched.plan();
    assert_eq!(
        plan.start,
        doc["expected_first_plan"]["start"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        plan.wait_dependencies,
        doc["expected_first_plan"]["wait_dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    );

    sched.start("WS024-A", "exec-a").unwrap();
    sched.start("WS024-B", "exec-b").unwrap();
    sched.set_pause(PauseGate::UserPaused);
    assert!(sched.plan().start.is_empty());
    sched.set_pause(PauseGate::Running);
    sched.complete("WS024-A", true, "ref://a").unwrap();
    sched.complete("WS024-B", true, "ref://b").unwrap();
    let plan = sched.plan();
    assert_eq!(plan.start, vec!["WS024-C"]);
    sched.start("WS024-C", "exec-c").unwrap();
    sched.mark_unknown("WS024-C").unwrap();
    let exit = sched.parent_session_exit();
    assert!(!exit.children_auto_completed);
    assert!(!exit.unknown_children_released);
    assert!(exit.retained_child_work_ids.contains(&"WS024-C".into()));
    let retry = sched.reconnect_before_retry("WS024-C").unwrap();
    assert!(!retry.duplicate_start_allowed);
    let refs = rollup_refs(&sched).unwrap();
    assert!(refs.iter().all(|r| !r.artifacts_copied));
}

#[test]
fn coordination_facts_are_not_process_control() {
    for fact in [
        "awr_admission_is_not_process_start",
        "awr_cancel_request_is_not_process_kill",
        "awr_session_end_is_not_process_kill",
    ] {
        assert!(matches!(
            refuse_coordination_as_process_control(fact),
            AdapterActionOutcome::CoordinationOnly { .. }
        ));
    }
}
