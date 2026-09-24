mod support;
use awr_core::*;
use support::Fixture;

fn digest(n: u8) -> String {
    format!("{n:064x}")
}

#[test]
fn sqlite_propose_accept_roundtrip() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    f.store.ensure_person(project, &alice, "Alice").unwrap();
    f.store.ensure_person(project, &bob, "Bob").unwrap();
    let package = HandoffPackage {
        task_id: "work-a".into(),
        contract_version: "1".into(),
        contract_hash: digest(1),
        current_person_id: alice.clone(),
        current_execution: ExecutionInstance::Person {
            person_id: alice.clone(),
        },
        consumed_context_digest: digest(2),
        checkpoint_ids: vec!["cp-1".into()],
        artifact_versions: vec![],
        branch_id: None,
        working_directory: None,
        dependency_ids: vec![],
        todos: vec!["t".into()],
        awaiting_replies: vec![],
        unknown_side_effects: vec![],
    };
    let (h, _) = f
        .store
        .propose_team_handoff(
            project,
            "work-a",
            &alice,
            &ProposeHandoffRequest {
                request_key: "p1".into(),
                handoff_id: "ho1".into(),
                kind: HandoffKind::Execution,
                package,
                to_person_id: bob.clone(),
                proposed_successor: None,
                proposer_execution_id: None,
                proposer_fence: None,
                expires_at_ms: None,
                now_ms: 1,
            },
        )
        .unwrap();
    assert_eq!(h.status, HandoffStatus::Proposed);
    let (h, receipt) = f
        .store
        .accept_team_handoff(
            project,
            &AcceptHandoffRequest {
                request_key: "a1".into(),
                handoff_id: "ho1".into(),
                expected_version: 1,
                acceptor_person_id: bob.clone(),
                successor_execution: ExecutionInstance::Person {
                    person_id: bob.clone(),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: None,
                live_fence: None,
                unknown_executions_open: false,
                now_ms: 2,
            },
        )
        .unwrap();
    assert_eq!(h.status, HandoffStatus::Accepted);
    assert!(!receipt.replayed);
    let (_, replay) = f
        .store
        .accept_team_handoff(
            project,
            &AcceptHandoffRequest {
                request_key: "a1".into(),
                handoff_id: "ho1".into(),
                expected_version: 1,
                acceptor_person_id: bob.clone(),
                successor_execution: ExecutionInstance::Person {
                    person_id: bob.clone(),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: None,
                live_fence: None,
                unknown_executions_open: false,
                now_ms: 3,
            },
        )
        .unwrap();
    assert!(replay.replayed);
}
