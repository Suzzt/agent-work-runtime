mod support;
use awr_core::*;
use support::Fixture;

#[test]
fn assign_claim_does_not_steal_owner_and_receipts_are_idempotent() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let work = "work-a";
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    f.store.ensure_person(project, &alice, "Alice").unwrap();
    f.store.ensure_person(project, &bob, "Bob").unwrap();

    let (task, receipt) = f
        .store
        .assign_responsibility(
            project,
            work,
            &AssignResponsibilityRequest {
                request_key: "asg-1".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![bob.clone()],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice.clone(),
            },
        )
        .unwrap();
    assert_eq!(task.owner, Some(alice.clone()));
    assert_eq!(task.collaborators, vec![bob.clone()]);
    assert!(task.current_executor.is_none());
    assert!(!receipt.replayed);

    let (_, replay) = f
        .store
        .assign_responsibility(
            project,
            work,
            &AssignResponsibilityRequest {
                request_key: "asg-1".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![bob.clone()],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice.clone(),
            },
        )
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.event_id, receipt.event_id);

    let (claimed, _) = f
        .store
        .claim_execution_responsibility(
            project,
            work,
            &ClaimExecutionRequest {
                request_key: "exec-1".into(),
                expected_version: task.version,
                executor: ExecutionInstance::Person {
                    person_id: bob.clone(),
                },
                coordination_claim_id: Some("coord-1".into()),
            },
        )
        .unwrap();
    assert_eq!(claimed.owner, Some(alice.clone()));
    assert_eq!(claimed.current_executor.unwrap().person_id(), &bob);

    let (released, _) = f
        .store
        .release_execution_responsibility(project, work, "rel-1", claimed.version, &bob)
        .unwrap();
    assert!(released.current_executor.is_none());
    assert_eq!(released.owner, Some(alice));

    let events = f.store.responsibility_events(project, work).unwrap();
    assert!(events.len() >= 3);
}

#[test]
fn agent_swap_keeps_ownership_with_explicit_binding() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let work = "work-b";
    let alice = PersonId::new("alice").unwrap();
    f.store.ensure_person(project, &alice, "Alice").unwrap();
    f.store
        .bind_person_agent(
            project,
            &PersonAgentBinding {
                id: "bind-old".into(),
                person_id: alice.clone(),
                agent_id: "agent-old".into(),
                status: BindingStatus::Active,
                created_at_ms: 1,
            },
        )
        .unwrap();
    f.store
        .bind_person_agent(
            project,
            &PersonAgentBinding {
                id: "bind-new".into(),
                person_id: alice.clone(),
                agent_id: "agent-new".into(),
                status: BindingStatus::Active,
                created_at_ms: 2,
            },
        )
        .unwrap();
    let (assigned, _) = f
        .store
        .assign_responsibility(
            project,
            work,
            &AssignResponsibilityRequest {
                request_key: "asg".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice.clone(),
            },
        )
        .unwrap();
    let (claimed, _) = f
        .store
        .claim_execution_responsibility(
            project,
            work,
            &ClaimExecutionRequest {
                request_key: "c-old".into(),
                expected_version: assigned.version,
                executor: ExecutionInstance::AgentRun {
                    person_id: alice.clone(),
                    agent_id: "agent-old".into(),
                    binding_id: "bind-old".into(),
                },
                coordination_claim_id: None,
            },
        )
        .unwrap();
    let (swapped, _) = f
        .store
        .swap_agent_for_person(
            project,
            work,
            "swap-1",
            claimed.version,
            &alice,
            ExecutionInstance::AgentRun {
                person_id: alice.clone(),
                agent_id: "agent-new".into(),
                binding_id: "bind-new".into(),
            },
        )
        .unwrap();
    assert_eq!(swapped.owner, Some(alice));
    match swapped.current_executor.unwrap() {
        ExecutionInstance::AgentRun { agent_id, .. } => assert_eq!(agent_id, "agent-new"),
        _ => panic!("expected agent run"),
    }
}

#[test]
fn unassigned_pool_and_transfer_pending() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let work = "work-c";
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    let carol = PersonId::new("carol").unwrap();
    f.store.ensure_person(project, &alice, "Alice").unwrap();
    f.store.ensure_person(project, &bob, "Bob").unwrap();
    f.store.ensure_person(project, &carol, "Carol").unwrap();
    let (claimed, _) = f
        .store
        .claim_execution_responsibility(
            project,
            work,
            &ClaimExecutionRequest {
                request_key: "pool-claim".into(),
                expected_version: 0,
                executor: ExecutionInstance::Person {
                    person_id: alice.clone(),
                },
                coordination_claim_id: None,
            },
        )
        .unwrap();
    assert!(claimed.owner.is_none());

    let (assigned, _) = f
        .store
        .assign_responsibility(
            project,
            work,
            &AssignResponsibilityRequest {
                request_key: "own".into(),
                expected_version: claimed.version,
                owner: Some(alice.clone()),
                collaborators: vec![],
                independent_reviewer: Some(carol),
                allow_unassigned: false,
                authorized_by: alice.clone(),
            },
        )
        .unwrap();
    let (xfer, _) = f
        .store
        .transfer_owner_responsibility(
            project,
            work,
            &TransferOwnerRequest {
                request_key: "xfer".into(),
                expected_version: assigned.version,
                from_owner: alice,
                to_owner: bob.clone(),
                authorized_by: PersonId::new("alice").unwrap(),
            },
        )
        .unwrap();
    assert_eq!(xfer.owner, Some(bob.clone()));
    assert_eq!(
        xfer.pending.as_ref().unwrap().kind,
        ResponsibilityPendingKind::NoAcceptor
    );
    let (accepted, _) = f
        .store
        .accept_responsibility(
            project,
            work,
            &AcceptResponsibilityRequest {
                request_key: "xfer-acc".into(),
                expected_version: xfer.version,
                acceptor: bob,
                as_owner: true,
            },
        )
        .unwrap();
    assert!(accepted.pending.is_none());
}

#[test]
fn request_key_cannot_replay_onto_a_different_work_item() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let alice = PersonId::new("alice").unwrap();
    f.store.ensure_person(project, &alice, "Alice").unwrap();
    f.store
        .assign_responsibility(
            project,
            "work-a",
            &AssignResponsibilityRequest {
                request_key: "same-key".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice.clone(),
            },
        )
        .unwrap();
    let err = f
        .store
        .assign_responsibility(
            project,
            "work-b",
            &AssignResponsibilityRequest {
                request_key: "same-key".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice,
            },
        )
        .unwrap_err();
    assert!(err.to_string().contains("different work item"), "{err}");
    let other = f.store.task_responsibility(project, "work-b").unwrap();
    assert!(other.owner.is_none());
    assert_eq!(other.version, 0);
}
