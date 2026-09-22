#![cfg(feature = "pg-tests")]
mod common;
use awr_core::*;
use awr_team_pg::HandoffStore;
use common::{fresh_team_schema, test_config, with_app_role};
use std::sync::MutexGuard;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (MutexGuard<'static, ()>, HandoffStore) {
    let (guard, admin, db) = fresh_team_schema().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');",
        )
        .await
        .unwrap();
    let cfg = with_app_role(&test_config(), &db);
    (guard, HandoffStore::from_config(cfg))
}

fn digest(n: u8) -> String {
    format!("{n:064x}")
}

fn package(from: &str) -> HandoffPackage {
    HandoffPackage {
        task_id: "work-a".into(),
        contract_version: "3".into(),
        contract_hash: digest(1),
        current_person_id: PersonId::new(from).unwrap(),
        current_execution: ExecutionInstance::Person {
            person_id: PersonId::new(from).unwrap(),
        },
        consumed_context_digest: digest(2),
        checkpoint_ids: vec!["cp-1".into()],
        artifact_versions: vec![],
        branch_id: None,
        working_directory: Some("crates/awr-core".into()),
        dependency_ids: vec![],
        todos: vec!["finish".into()],
        awaiting_replies: vec![],
        unknown_side_effects: vec![],
    }
}

#[tokio::test]
async fn propose_inspect_accept_reject_timeout_roundtrip() {
    let (_g, store) = setup().await;
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    let (h, receipt) = store
        .propose(
            TENANT,
            PROJECT,
            "work-a",
            &alice,
            &ProposeHandoffRequest {
                request_key: "prop-1".into(),
                handoff_id: "ho-1".into(),
                kind: HandoffKind::Execution,
                package: package("alice"),
                to_person_id: bob.clone(),
                proposed_successor: Some(ExecutionInstance::Person {
                    person_id: bob.clone(),
                }),
                proposer_execution_id: None,
                proposer_fence: Some(1),
                expires_at_ms: Some(10_000),
                now_ms: 1_000,
            },
        )
        .await
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(h.status, HandoffStatus::Proposed);

    let (h, _) = store
        .inspect(
            TENANT,
            PROJECT,
            &InspectHandoffRequest {
                request_key: "ins-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 1,
                inspector_person_id: bob.clone(),
                now_ms: 1_500,
            },
        )
        .await
        .unwrap();
    assert_eq!(h.status, HandoffStatus::Inspected);

    let (h, _) = store
        .accept(
            TENANT,
            PROJECT,
            &AcceptHandoffRequest {
                request_key: "acc-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 2,
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
                now_ms: 2_000,
            },
        )
        .await
        .unwrap();
    assert_eq!(h.status, HandoffStatus::Accepted);
    let duty = store.duty(TENANT, PROJECT, "ho-1", 2_000).await.unwrap();
    assert!(duty.successor_may_execute);
    assert_eq!(duty.responsible_person_id, alice);

    // Idempotent replay
    let (_, replay) = store
        .accept(
            TENANT,
            PROJECT,
            &AcceptHandoffRequest {
                request_key: "acc-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 2,
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
                now_ms: 3_000,
            },
        )
        .await
        .unwrap();
    assert!(replay.replayed);
}

#[tokio::test]
async fn timeout_preserves_original_without_stop() {
    let (_g, store) = setup().await;
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    store
        .propose(
            TENANT,
            PROJECT,
            "work-a",
            &alice,
            &ProposeHandoffRequest {
                request_key: "prop-to".into(),
                handoff_id: "ho-to".into(),
                kind: HandoffKind::Execution,
                package: package("alice"),
                to_person_id: bob,
                proposed_successor: None,
                proposer_execution_id: None,
                proposer_fence: None,
                expires_at_ms: Some(5_000),
                now_ms: 1_000,
            },
        )
        .await
        .unwrap();
    let (h, _) = store
        .timeout(
            TENANT,
            PROJECT,
            &TimeoutHandoffRequest {
                request_key: "to-1".into(),
                handoff_id: "ho-to".into(),
                expected_version: 1,
                now_ms: 5_000,
            },
        )
        .await
        .unwrap();
    assert_eq!(h.status, HandoffStatus::TimedOut);
    let duty = h.duty_at(5_000).unwrap();
    assert_eq!(duty.responsible_person_id, alice);
    assert!(!duty.successor_may_execute);
    assert!(duty.note.contains("not stop"));
}
