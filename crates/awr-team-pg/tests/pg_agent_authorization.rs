#![cfg(feature = "pg-tests")]
mod common;
use awr_core::*;
use awr_team_pg::{AuthorizationStore, ResponsibilityStore};
use common::{fresh_team_schema, test_config, with_app_role};
use std::collections::BTreeSet;
use std::sync::MutexGuard;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (MutexGuard<'static, ()>, AuthorizationStore, ResponsibilityStore) {
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
    (
        guard,
        AuthorizationStore::from_config(cfg.clone()),
        ResponsibilityStore::from_config(cfg),
    )
}

fn sample(person: &PersonId) -> AgentAuthorization {
    AgentAuthorization {
        id: "auth-1".into(),
        authorizer_person_id: person.clone(),
        responsible_person_id: person.clone(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent-1".into(),
        client_id: "client-1".into(),
        session_id: Some("sess-1".into()),
        model_id: Some("model-a".into()),
        scope: AuthorizationScope::Project {
            project_id: PROJECT.into(),
        },
        actions: BTreeSet::from([
            AuthorizedAction::OccupyCollaboratively,
            AuthorizedAction::StartWork,
            AuthorizedAction::AcceptResponsibility,
            AuthorizedAction::ManageAuthorization,
        ]),
        expires_at_ms: Some(10_000),
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![],
        self_reported_skill_hints: vec!["hint".into()],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1_000,
        binding_id: Some("bind-1".into()),
    }
}

#[tokio::test]
async fn issue_list_revoke_roundtrip() {
    let (_g, store, people) = setup().await;
    let alice = PersonId::new("alice").unwrap();
    people
        .ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    let auth = sample(&alice);
    let (stored, receipt) = store
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "iss-1".into(),
                authorization: auth,
            },
        )
        .await
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(stored.id, "auth-1");
    let listed = store
        .list(TENANT, PROJECT, Some(&alice), None, true)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    let (revoked, _) = store
        .revoke(
            TENANT,
            PROJECT,
            &RevokeAuthorizationRequest {
                request_key: "rev-1".into(),
                authorization_id: "auth-1".into(),
                revoked_by: alice,
                revoked_at_ms: 3_000,
                reason: "done".into(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(revoked.status, AuthorizationStatus::Revoked));
}
