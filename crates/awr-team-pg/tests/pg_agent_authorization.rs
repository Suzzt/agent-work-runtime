#![cfg(feature = "pg-tests")]
mod common;
use awr_core::*;
use awr_team_pg::{AuthorizationStore, ResponsibilityStore};
use common::{app_client, fresh_team_schema, test_config, with_app_role};
use std::collections::BTreeSet;
use std::sync::MutexGuard;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (
    MutexGuard<'static, ()>,
    AuthorizationStore,
    ResponsibilityStore,
    String,
) {
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
        db,
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
    let (_g, store, people, _db) = setup().await;
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
                revoked_by: alice.clone(),
                revoked_at_ms: 3_000,
                reason: "done".into(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(revoked.status, AuthorizationStatus::Revoked));

    let mut changed = stored.clone();
    changed.client_id = "other-client".into();
    let changed_issue = store
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "iss-1".into(),
                authorization: changed,
            },
        )
        .await;
    assert!(
        matches!(
            changed_issue,
            Err(awr_team_pg::PgError::IdempotencyConflict)
        ),
        "same request key with a different grant must conflict: {changed_issue:?}"
    );
    let mut foreign = sample(&alice);
    foreign.id = "auth-foreign".into();
    foreign.scope = AuthorizationScope::Project {
        project_id: "other-project".into(),
    };
    let denied = store
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "iss-foreign".into(),
                authorization: foreign,
            },
        )
        .await;
    assert!(
        matches!(denied, Err(awr_team_pg::PgError::Forbidden)),
        "scope project must match the addressed project: {denied:?}"
    );
}

#[tokio::test]
async fn rls_hides_authorizations_without_tenant_scope() {
    let (_g, store, people, db) = setup().await;
    let alice = PersonId::new("alice").unwrap();
    people
        .ensure_person(TENANT, PROJECT, alice.as_str(), "Alice")
        .await
        .unwrap();
    store
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "iss-rls".into(),
                authorization: sample(&alice),
            },
        )
        .await
        .unwrap();

    let mut app = app_client(&db).await;
    let unscoped: i64 = app
        .query_one("SELECT count(*) FROM awr_team.agent_authorizations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(unscoped, 0, "unscoped app role must not see grants");

    let tx = app.transaction().await.unwrap();
    tx.execute("SELECT set_config('awr.tenant_id', 'tenant-b', true)", &[])
        .await
        .unwrap();
    tx.execute(
        "SELECT set_config('awr.project_id', 'project-b', true)",
        &[],
    )
    .await
    .unwrap();
    let cross: i64 = tx
        .query_one("SELECT count(*) FROM awr_team.agent_authorizations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(cross, 0, "wrong tenant must not see grants");
    tx.rollback().await.unwrap();

    let scoped = app.transaction().await.unwrap();
    scoped
        .execute("SELECT set_config('awr.tenant_id', $1, true)", &[&TENANT])
        .await
        .unwrap();
    scoped
        .execute("SELECT set_config('awr.project_id', $1, true)", &[&PROJECT])
        .await
        .unwrap();
    let visible: i64 = scoped
        .query_one("SELECT count(*) FROM awr_team.agent_authorizations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(visible, 1);
    assert!(
        scoped
            .execute(
                "UPDATE awr_team.agent_authorization_receipts SET op='tamper'",
                &[]
            )
            .await
            .is_err(),
        "receipts are append-only for the app role"
    );
    scoped.rollback().await.unwrap();
}
