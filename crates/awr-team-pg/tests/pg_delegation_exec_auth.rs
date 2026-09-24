//! AWR-TMCP-030: agent delegation ∩ TMCP product permissions on real PG paths.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_core::*;
use awr_team_pg::{AuthorizationStore, PgError};
use fixture::*;
use serde_json::json;
use std::collections::BTreeSet;

async fn flip_actor_to_agent(admin: &tokio_postgres::Client) {
    admin
        .batch_execute(
            "UPDATE awr_team.actors SET kind='agent' WHERE tenant_id='reader-tenant' AND id='agent';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status)
               VALUES('reader-tenant','reader-project','alice','Alice','active')
               ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
               VALUES('reader-tenant','reader-project','bind-agent','alice','agent','active')
               ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
}

fn work_grant(person: &PersonId) -> AgentAuthorization {
    AgentAuthorization {
        id: "auth-agent-work".into(),
        authorizer_person_id: person.clone(),
        responsible_person_id: person.clone(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent".into(),
        client_id: "cli-a".into(),
        session_id: None,
        model_id: None,
        scope: AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
        actions: BTreeSet::from([
            AuthorizedAction::StartWork,
            AuthorizedAction::ClaimCoordination,
            AuthorizedAction::Inspect,
        ]),
        expires_at_ms: None,
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec![],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1_000,
        binding_id: Some("bind-agent".into()),
    }
}

#[tokio::test]
async fn admin_membership_agent_without_delegation_is_forbidden() {
    let (_guard, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    flip_actor_to_agent(&admin).await;
    let commands = store.commands();
    let err = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-no-deleg",
                "session.start",
                json!({"conversation_id": "c-denied"}),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
}

#[tokio::test]
async fn explicit_start_work_delegation_allows_session() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    flip_actor_to_agent(&admin).await;
    let authz = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let alice = PersonId::new("alice").unwrap();
    authz
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "iss-agent-work".into(),
                authorization: work_grant(&alice),
            },
        )
        .await
        .unwrap();

    let commands = store.commands();
    let started = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-with-deleg",
                "session.start",
                json!({"conversation_id": "c-allowed"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(started["replayed"], false);
}

#[tokio::test]
async fn disabled_binding_or_person_blocks_delegated_session() {
    let (_guard, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    flip_actor_to_agent(&admin).await;
    let authz = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let alice = PersonId::new("alice").unwrap();
    authz
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "iss-agent-disabled-bind".into(),
                authorization: work_grant(&alice),
            },
        )
        .await
        .unwrap();

    // Disable the binding while authorization JSON remains active.
    admin
        .batch_execute(
            "UPDATE awr_team.person_agent_bindings SET status='disabled'
             WHERE id='bind-agent';",
        )
        .await
        .unwrap();
    let commands = store.commands();
    let err = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-disabled-bind",
                "session.start",
                json!({"conversation_id": "c-disabled-bind"}),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");

    // Re-enable binding but disable the person.
    admin
        .batch_execute(
            "UPDATE awr_team.person_agent_bindings SET status='active' WHERE id='bind-agent';
             UPDATE awr_team.persons SET status='disabled' WHERE id='alice';",
        )
        .await
        .unwrap();
    let err = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-disabled-person",
                "session.start",
                json!({"conversation_id": "c-disabled-person"}),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");

    // Valid binding+person still works (control).
    admin
        .batch_execute("UPDATE awr_team.persons SET status='active' WHERE id='alice';")
        .await
        .unwrap();
    let started = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "agent-live-ok",
                "session.start",
                json!({"conversation_id": "c-live-ok"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(started["replayed"], false);
}
