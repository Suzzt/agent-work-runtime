#![cfg(feature = "pg-tests")]
//! WS-015 responsibility / identity / assignment on real PostgreSQL.
mod common;
use awr_core::*;
use awr_team_pg::ResponsibilityStore;
use common::{app_client, fresh_team_schema, test_config, with_app_role};
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

#[tokio::test]
async fn responsibility_rls_blocks_unscoped_and_cross_tenant_app_reads() {
    let (_g, admin, store) = setup().await;
    // Seed a second tenant/project as superuser (bypasses FORCE RLS).
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-b','B','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-b','project-b','beta','team','epoch-1','active');
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
                VALUES ('tenant-b','project-b','eve','Eve','active');
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
                VALUES ('tenant-a','project-a','alice','Alice','active');",
        )
        .await
        .unwrap();

    // Store path with correct tenant scope can read its own person via get after assign.
    let alice = PersonId::new("alice").unwrap();
    store
        .assign(
            TENANT,
            PROJECT,
            "work-rls",
            &AssignResponsibilityRequest {
                request_key: "rls-asg".into(),
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
    let mine = store.get(TENANT, PROJECT, "work-rls").await.unwrap();
    assert_eq!(mine.owner, Some(alice));

    // Unscoped app role must not see any persons (including other tenants).
    let mut app = {
        // Reconnect as awr_app against the same DB the store uses.
        let url = std::env::var("AWR_TEAM_TEST_DATABASE_URL").ok();
        let _ = url;
        // Pull db name from admin connection via current_database.
        let db: String = admin
            .query_one("SELECT current_database()", &[])
            .await
            .unwrap()
            .get(0);
        app_client(&db).await
    };
    let leaked: i64 = app
        .query_one("SELECT count(*) FROM awr_team.persons", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        leaked, 0,
        "unscoped app must not read persons across tenants"
    );

    // Wrong-tenant scope must not reveal tenant-b rows.
    let tx = app.transaction().await.unwrap();
    tx.execute("SELECT set_config('awr.tenant_id', $1, true)", &[&TENANT])
        .await
        .unwrap();
    tx.execute("SELECT set_config('awr.project_id', $1, true)", &[&PROJECT])
        .await
        .unwrap();
    let cross: i64 = tx
        .query_one(
            "SELECT count(*) FROM awr_team.persons WHERE tenant_id='tenant-b'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(cross, 0);
    let visible: i64 = tx
        .query_one("SELECT count(*) FROM awr_team.persons", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(visible, 1);
    tx.commit().await.unwrap();

    // Append-only: app cannot UPDATE/DELETE responsibility_events / receipts.
    let scoped = app.transaction().await.unwrap();
    scoped
        .execute("SELECT set_config('awr.tenant_id', $1, true)", &[&TENANT])
        .await
        .unwrap();
    scoped
        .execute("SELECT set_config('awr.project_id', $1, true)", &[&PROJECT])
        .await
        .unwrap();
    assert!(
        scoped
            .execute(
                "UPDATE awr_team.responsibility_events SET event_type='tamper'",
                &[]
            )
            .await
            .is_err()
    );
    assert!(
        scoped
            .execute("DELETE FROM awr_team.responsibility_receipts", &[])
            .await
            .is_err()
    );
    scoped.rollback().await.unwrap();
}

#[tokio::test]
async fn request_key_cannot_replay_onto_a_different_work_item() {
    let (_g, _admin, store) = setup().await;
    let alice = PersonId::new("alice").unwrap();
    store
        .ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    store
        .assign(
            TENANT,
            PROJECT,
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
        .await
        .unwrap();
    let err = store
        .assign(
            TENANT,
            PROJECT,
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
        .await
        .unwrap_err();
    assert!(matches!(err, awr_team_pg::PgError::IdempotencyConflict));
    let other = store.get(TENANT, PROJECT, "work-b").await.unwrap();
    assert!(other.owner.is_none());
    assert_eq!(other.version, 0);
}

#[tokio::test]
async fn concurrent_first_assigns_do_not_overwrite_each_other() {
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
    let alice_req = AssignResponsibilityRequest {
        request_key: "race-alice".into(),
        expected_version: 0,
        owner: Some(alice.clone()),
        collaborators: vec![],
        independent_reviewer: None,
        allow_unassigned: false,
        authorized_by: alice.clone(),
    };
    let bob_req = AssignResponsibilityRequest {
        request_key: "race-bob".into(),
        expected_version: 0,
        owner: Some(bob.clone()),
        collaborators: vec![],
        independent_reviewer: None,
        allow_unassigned: false,
        authorized_by: bob.clone(),
    };
    let left = store.assign(TENANT, PROJECT, "work-race", &alice_req);
    let right = store.assign(TENANT, PROJECT, "work-race", &bob_req);
    let (left, right) = tokio::join!(left, right);
    let wins = [&left, &right].iter().filter(|r| r.is_ok()).count();
    assert_eq!(wins, 1, "left={left:?} right={right:?}");
    let task = store.get(TENANT, PROJECT, "work-race").await.unwrap();
    assert_eq!(task.version, 1);
    assert!(task.owner == Some(alice) || task.owner == Some(bob));
}
