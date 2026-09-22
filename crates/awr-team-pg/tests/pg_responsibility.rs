#![cfg(feature = "pg-tests")]
//! WS-015 responsibility / identity / assignment on real PostgreSQL.
mod common;
use awr_core::*;
use awr_team_pg::ResponsibilityStore;
use common::{fresh_team_schema, test_config, with_app_role};
use std::sync::MutexGuard;
use tokio_postgres::Client;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (MutexGuard<'static, ()>, Client, ResponsibilityStore) {
    let (guard, admin, db) = fresh_team_schema().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');",
        )
        .await
        .unwrap();
    let store = ResponsibilityStore::from_config(with_app_role(&test_config(), &db));
    (guard, admin, store)
}

#[tokio::test]
async fn claim_does_not_steal_ownership_and_receipts_replay() {
    let (_g, _admin, store) = setup().await;
    let alice = PersonId::new("alice").unwrap();
    let bob = PersonId::new("bob").unwrap();
    store
        .ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    store
        .ensure_person(TENANT, PROJECT, bob.as_str(), "Bob")
        .await
        .unwrap();
    let (assigned, receipt) = store
        .assign(
            TENANT,
            PROJECT,
            "work-a",
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
        .await
        .unwrap();
    assert_eq!(assigned.owner, Some(alice.clone()));
    let (_, replay) = store
        .assign(
            TENANT,
            PROJECT,
            "work-a",
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
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.event_id, receipt.event_id);

    let (claimed, _) = store
        .claim_execution(
            TENANT,
            PROJECT,
            "work-a",
            &ClaimExecutionRequest {
                request_key: "exec-1".into(),
                expected_version: assigned.version,
                executor: ExecutionInstance::Person {
                    person_id: bob.clone(),
                },
                coordination_claim_id: Some("coord-1".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(claimed.owner, Some(alice));
    assert_eq!(claimed.current_executor.unwrap().person_id(), &bob);
}

#[tokio::test]
async fn agent_swap_requires_explicit_binding_not_actor_kind() {
    let (_g, admin, store) = setup().await;
    // Seed an actor.kind=agent that must NOT be treated as a person↔agent binding.
    admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
             VALUES ('tenant-a','agent-ghost','agent','Ghost','active');",
        )
        .await
        .unwrap();
    let alice = PersonId::new("alice").unwrap();
    store
        .ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    let err = store
        .claim_execution(
            TENANT,
            PROJECT,
            "work-b",
            &ClaimExecutionRequest {
                request_key: "bad".into(),
                expected_version: 0,
                executor: ExecutionInstance::AgentRun {
                    person_id: alice.clone(),
                    agent_id: "agent-ghost".into(),
                    binding_id: "missing".into(),
                },
                coordination_claim_id: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, awr_team_pg::PgError::Forbidden));

    store
        .bind_person_agent(
            TENANT,
            PROJECT,
            &PersonAgentBinding {
                id: "bind-1".into(),
                person_id: alice.clone(),
                agent_id: "agent-ghost".into(),
                status: BindingStatus::Active,
                created_at_ms: 1,
            },
        )
        .await
        .unwrap();
    let (assigned, _) = store
        .assign(
            TENANT,
            PROJECT,
            "work-b",
            &AssignResponsibilityRequest {
                request_key: "own".into(),
                expected_version: 0,
                owner: Some(alice.clone()),
                collaborators: vec![],
                independent_reviewer: None,
                allow_unassigned: false,
                authorized_by: alice.clone(),
            },
        )
        .await
        .unwrap();
    let (claimed, _) = store
        .claim_execution(
            TENANT,
            PROJECT,
            "work-b",
            &ClaimExecutionRequest {
                request_key: "ok".into(),
                expected_version: assigned.version,
                executor: ExecutionInstance::AgentRun {
                    person_id: alice.clone(),
                    agent_id: "agent-ghost".into(),
                    binding_id: "bind-1".into(),
                },
                coordination_claim_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(claimed.owner, Some(alice));
}
