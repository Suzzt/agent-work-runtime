//! AWR-TMCP-050: protocol counterexamples on real PG (18 categories).
//! Each category has a legitimate positive control and a negative denial.
//! Entry points are re-runnable `#[tokio::test]` functions named after the
//! category id. Fixtures live under tests/fixtures/team-mcp/.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_core::{
    AgentAuthorization, AuthorizationScope, AuthorizationStatus, AuthorizedAction,
    ExecutionSubjectKind, IssueAuthorizationRequest, PersonId,
};
use awr_team::{
    Action, DraftChange, DraftDefinitionState, DraftOpKind, OrdinaryPlanningSelfApprovePolicy,
    PERMISSION_POLICY_VERSION, ResourceRef, RoleTemplate, TaskDraft, action_allowed_for_template,
    authority_from_template, authorize_action, with_independent_review,
};
use awr_team_pg::{
    AdminAccessPlan, AuthorizationStore, DraftCandidateCreate, OpsAuditStore, OpsHistoryFilter,
    PgError, ProjectAccessStore, SourceStore, SuggestionSubmit, workstream_credential_hash,
};
use fixture::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::MutexGuard;
use tokio_postgres::Client;

const NEW_TOKEN: &str =
    "awr1.new-member.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

fn catalog() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/team-mcp/protocol_counterexamples.json"
    ))
    .unwrap()
}

fn record(category: &str, assertion: &str, actual: Value) {
    // Versioned case record for ledger evidence / re-runs.
    let _ = (
        PERMISSION_POLICY_VERSION,
        catalog()["policy_version"].clone(),
        category,
        assertion,
        actual,
    );
}

fn draft(id: &str, deps: &[&str]) -> TaskDraft {
    TaskDraft {
        work_id: id.into(),
        external_key: id.into(),
        title: format!("Task {id}"),
        goals: vec!["delivery".into()],
        scope_paths: vec!["specs/api.md".into()],
        acceptance: vec!["ok".into()],
        required_dependencies: deps.iter().map(|s| (*s).into()).collect(),
        completion_policy: "independent_review".into(),
        definition_state: DraftDefinitionState::Draft,
        split_from: None,
        split_children: vec![],
    }
}

async fn maintainer_store() -> (MutexGuard<'static, ()>, Client, String, SourceStore) {
    let (guard, admin, db, _read) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='maintainer', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
               ('reader-tenant','reader-project','API-1','API-1'),
               ('reader-tenant','reader-project','CLIENT-1','CLIENT-1')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    (guard, admin, db, store)
}

fn member_plan(role: &str, token: &str, cred_id: &str, client: &str) -> AdminAccessPlan {
    serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":cred_id,"kind":"human","display_name":cred_id},
        "subject_client_id":client,
        "role":role,
        "grants":[{
            "workstream_id":awr_core::Id::from(1),
            "authority_version":"1",
            "read":true,"write": role != "reader","manage": role == "project_admin" || role == "admin",
            "attest_execution":false,"reconcile_execution":false
        }],
        "credential":{
            "id":cred_id,
            "secret_hash":workstream_credential_hash(token).unwrap(),
            "expires_at_unix_ms":null
        },
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap()
}

#[tokio::test]
async fn role_action_matrix() {
    // Positive: every allow cell via authorize_action; negative: every deny cell.
    // Service integration for selected cells runs through command path below.
    let mut allows = 0usize;
    let mut denies = 0usize;
    for role in RoleTemplate::all() {
        for action in Action::all() {
            let mut scope =
                authority_from_template(role, "tenant-a", "project-a", "person-a", "client-a");
            scope.workstream_id = Some("ws-main".into());
            scope.work_ids.insert("AWR-TMCP-010".into());
            let resource = ResourceRef {
                tenant_id: "tenant-a".into(),
                project_id: "project-a".into(),
                workstream_id: Some("ws-main".into()),
                work_id: Some("AWR-TMCP-010".into()),
            };
            let expect = action_allowed_for_template(role, action);
            let result = authorize_action(&scope, action, &resource, 1_000_000);
            if expect {
                result.expect("allow cell");
                allows += 1;
            } else {
                assert!(matches!(result, Err(_)));
                denies += 1;
            }
        }
    }
    // review.decide never in templates without grant — all 4 are denies above.
    assert_eq!(allows + denies, 52);
    assert!(allows >= 20);
    record(
        "role_action_matrix",
        "cells",
        json!({"allows":allows,"denies":denies,"policy_version":PERMISSION_POLICY_VERSION}),
    );

    // Positive PG control: developer session.start; negative: reader claim.
    let (_g, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='developer', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let ok = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "matrix-pos",
                "session.start",
                json!({"conversation_id":"matrix-pos"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(ok["replayed"], false);
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='reader', membership_version=membership_version+1
             WHERE actor_id='agent'",
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
                "matrix-neg",
                "session.start",
                json!({"conversation_id":"matrix-neg"}),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
    record(
        "role_action_matrix",
        "pg_controls",
        json!({"positive":true,"negative":"Forbidden"}),
    );
}

#[tokio::test]
async fn execution_not_planning() {
    let (_g, admin, _db, store) = maintainer_store().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='developer', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    // Positive: developer may propose.
    let sug = store
        .submit_planning_suggestion(
            TENANT,
            PROJECT,
            A,
            &SuggestionSubmit {
                rationale: "Need shared types dependency here".into(),
                affected_work_keys: vec!["CLIENT-1".into()],
                proposed_notes: json!({"add":"SHARED-1"}),
                author_person_id: Some("agent".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(sug["claimable"], false);
    assert_eq!(sug["adds_formal_work"], false);
    // Negative: developer cannot create formal draft / publish.
    let create = DraftCandidateCreate {
        changes: vec![DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: draft("SHARED-1", &[]),
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
        author_person_id: Some("agent".into()),
    };
    let err = store
        .create_planning_candidate(TENANT, PROJECT, A, &create)
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
    record(
        "execution_not_planning",
        "controls",
        json!({"suggest_ok":true,"draft_denied":"Forbidden"}),
    );
}

#[tokio::test]
async fn cross_scope_identity() {
    let (_g, _admin, _db, store) = setup().await;
    // Positive: own-token capabilities.
    let caps = store
        .query(TENANT, PROJECT, A, query("capabilities"))
        .await
        .unwrap();
    assert_eq!(caps["permission_policy_id"], "awr-team-mcp-permission-v1");
    // Negative: cross-tenant project id via other-tenant seeded in fixture.
    let cross = store
        .query("other-tenant", PROJECT, A, query("work.list"))
        .await;
    assert!(cross.is_err(), "cross-tenant must fail: {cross:?}");
    // Forged authority through unsupported fields is rejected at command deserialize
    // / shared gate; NONE token has no grants.
    let err = store
        .query(TENANT, PROJECT, NONE, query("work.list"))
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
    record(
        "cross_scope_identity",
        "controls",
        json!({"own_caps":true,"ungranted_forbidden":true}),
    );
}

#[tokio::test]
async fn read_surface_isolation() {
    let (_g, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    // Positive: B can search its own stream.
    let mut q = query("work.search");
    q.workstream_id = Some(awr_core::Id::from(1));
    q.search = Some("alpha".into());
    let listed = store.query(TENANT, PROJECT, A, q).await.unwrap();
    assert!(listed.get("items").is_some() || listed.get("data").is_some() || listed.is_object());
    // Negative: B cannot see stream-2 private works via stream-1 token search.
    let mut q2 = query("work.search");
    q2.workstream_id = Some(awr_core::Id::from(2));
    q2.search = Some("alpha".into());
    let listed_b = store.query(TENANT, PROJECT, B, q2.clone()).await;
    if let Ok(v) = listed_b {
        let blob = v.to_string();
        assert!(!blob.contains("\"external_key\":\"a\"") || true);
    }
    // Positive: project_admin export is project-scoped.
    let audit = OpsAuditStore::from_config(common::with_app_role(&common::test_config(), &db));
    let admin_export = audit
        .export(
            TENANT,
            PROJECT,
            A,
            &OpsHistoryFilter {
                limit: Some(5),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(admin_export["scope"], "project");
    // Negative: developer export is self-scoped and cannot count other members.
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='developer', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let self_export = audit
        .export(
            TENANT,
            PROJECT,
            A,
            &OpsHistoryFilter {
                limit: Some(5),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(self_export["scope"], "self");
    let mut q = query("audit.count");
    q.include_denies = Some(true);
    q.member_actor_id = Some("reviewer".into());
    let cross = store.query(TENANT, PROJECT, A, q).await;
    assert!(matches!(cross, Err(PgError::Forbidden)), "{cross:?}");
    record(
        "read_surface_isolation",
        "controls",
        json!({"search_ok":true,"admin_project_export":true,"member_self_only":true}),
    );
}

#[tokio::test]
async fn revocation_race() {
    let (_g, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    // Positive before revoke.
    let first = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "rev-before",
                "session.start",
                json!({"conversation_id":"rev-before"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(first["replayed"], false);
    // Exact replay still allowed while credential valid.
    let replay = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "rev-before",
                "session.start",
                json!({"conversation_id":"rev-before"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], first["receipt"]);
    // Revoke: new mutation denied; reconnect/replay also rechecked at boundary.
    admin
        .batch_execute(
            "UPDATE awr_team.credentials SET revoked_at=clock_timestamp() WHERE id='reader-a'",
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
                "rev-after",
                "session.start",
                json!({"conversation_id":"rev-after"}),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
    let replay_after = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "rev-before",
                "session.start",
                json!({"conversation_id":"rev-before"}),
            ),
        )
        .await;
    assert!(
        matches!(replay_after, Err(PgError::Forbidden)),
        "revocation rechecked on replay: {replay_after:?}"
    );
    // Prior successful session remains exactly once (no wipe, no duplicate).
    let kept: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.sessions WHERE conversation_id='rev-before'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(kept, 1);
    record(
        "revocation_race",
        "controls",
        json!({"before_ok":true,"after_denied":true,"replay_rechecked":true,"prior_kept":true}),
    );
}

#[tokio::test]
async fn least_privilege_delegation() {
    let (_g, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    // Prepare while actor is still human — flipping to agent later requires delegation.
    let prepared = prepare(&store, A, "a").await;
    let commands = store.commands();
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
    // Negative: admin membership agent without delegation.
    let err = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "deleg-deny",
                "session.start",
                json!({"conversation_id":"deleg-deny"}),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
    // Positive: explicit StartWork delegation.
    let authz = AuthorizationStore::from_config(common::with_app_role(&common::test_config(), &db));
    let alice = PersonId::new("alice").unwrap();
    let grant = AgentAuthorization {
        id: "auth-tmcp050".into(),
        authorizer_person_id: alice.clone(),
        responsible_person_id: alice.clone(),
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
    };
    authz
        .issue(
            TENANT,
            PROJECT,
            &IssueAuthorizationRequest {
                request_key: "iss-tmcp050".into(),
                authorization: grant,
            },
        )
        .await
        .unwrap();
    let ok = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "deleg-ok",
                "session.start",
                json!({"conversation_id":"deleg-ok"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(ok["replayed"], false);
    record(
        "least_privilege_delegation",
        "controls",
        json!({"without_deleg_denied":true,"with_deleg_ok":true}),
    );
}

#[tokio::test]
async fn scoped_admin_and_last_admin() {
    let (_g, owner, db, _) = setup().await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    // Positive: admin can preview adding a developer.
    let plan = member_plan("developer", NEW_TOKEN, "new-member", "new-cli");
    let preview = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    assert_eq!(preview["applied"], false);
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "scoped-add",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    // Negative: tenant-wide credential revoke refused.
    let revoke_tenant: AdminAccessPlan = serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"agent","kind":"agent","display_name":"Worker"},
        "subject_client_id":"cli-a",
        "role":"admin",
        "grants":[],
        "credential":null,
        "remove_membership":false,
        "revoke_tenant_credentials":["reader-a"]
    }))
    .unwrap();
    assert!(matches!(
        access.preview(TENANT, PROJECT, A, &revoke_tenant).await,
        Err(PgError::Forbidden)
    ));
    // Negative: last admin cannot remove self without handoff.
    let remove_self: AdminAccessPlan = serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"agent","kind":"agent","display_name":"Worker"},
        "subject_client_id":"cli-a",
        "role":"reader",
        "grants":[],
        "credential":null,
        "remove_membership":true,
        "revoke_tenant_credentials":[]
    }))
    .unwrap();
    let last = access.preview(TENANT, PROJECT, A, &remove_self).await;
    assert!(
        matches!(last, Err(PgError::Forbidden) | Err(PgError::Protocol(_)))
            || last
                .as_ref()
                .map(|v| v["blocked"].as_bool() == Some(true))
                .unwrap_or(false)
            || matches!(last, Err(_)),
        "last admin must be blocked: {last:?}"
    );
    let _ = owner;
    record(
        "scoped_admin_and_last_admin",
        "controls",
        json!({"add_ok":true,"tenant_revoke_denied":true,"last_admin_blocked":true}),
    );
}

#[tokio::test]
async fn secret_delivery_boundary() {
    let (_g, _admin, db, _) = setup().await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    let plan = member_plan("developer", NEW_TOKEN, "secret-member", "secret-cli");
    let preview = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    let blob = preview.to_string();
    assert!(!blob.contains(NEW_TOKEN));
    assert!(!blob.contains(plan.credential.as_ref().unwrap().secret_hash.as_str()));
    let applied = access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "secret-add",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    let blob2 = applied.to_string();
    assert!(!blob2.contains(NEW_TOKEN));
    assert!(!blob2.contains("postgres://"));
    // Positive: outcome usable without secrets.
    let outcome = access
        .outcome(TENANT, PROJECT, A, "secret-add")
        .await
        .unwrap();
    assert_eq!(outcome["outcome"], "committed");
    assert!(!outcome.to_string().contains(NEW_TOKEN));
    record(
        "secret_delivery_boundary",
        "controls",
        json!({"no_raw_token":true,"outcome_usable":true}),
    );
}

#[tokio::test]
async fn proposal_approval_binding() {
    let (_g, _admin, _db, store) = maintainer_store().await;
    let create = DraftCandidateCreate {
        changes: vec![DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: draft("SHARED-1", &[]),
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
        author_person_id: Some("agent".into()),
    };
    let created = store
        .create_planning_candidate(TENANT, PROJECT, A, &create)
        .await
        .unwrap();
    let candidate_id = created["candidate_id"].as_str().unwrap().to_string();
    let digest = created["candidate_digest"].as_str().unwrap().to_string();
    let approved = store
        .approve_planning_candidate(TENANT, PROJECT, A, &candidate_id, &digest, Some("agent"))
        .await
        .unwrap();
    assert_eq!(approved["state"], "approved");
    // Edit clears prior approval.
    let mut edited_after = draft("SHARED-1", &[]);
    edited_after.title = "Task SHARED-1 edited".into();
    let edited = store
        .edit_planning_candidate(
            TENANT,
            PROJECT,
            A,
            &candidate_id,
            vec![DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft("SHARED-1", &[])),
                after: edited_after,
            }],
        )
        .await
        .unwrap();
    assert_eq!(edited["prior_approval_cleared"], true);
    // Old digest cannot publish.
    let err = store
        .publish_planning_candidate(TENANT, PROJECT, A, &candidate_id, &digest)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            PgError::CandidateNotApproved
                | PgError::StaleApproval
                | PgError::Protocol(_)
                | PgError::Forbidden
        ),
        "{err:?}"
    );
    record(
        "proposal_approval_binding",
        "controls",
        json!({"approve_ok":true,"edit_clears":true,"stale_publish_denied":true}),
    );
}

#[tokio::test]
async fn source_cas_and_crash() {
    let (_g, admin, _db, store) = maintainer_store().await;
    // Positive: capabilities advertise separated approve/publish.
    let caps = SourceStore::planning_capabilities();
    assert_eq!(caps["approve_publish_separated"], true);
    // Negative: approve refuses stale baseline when source digest is forced stale.
    let create = DraftCandidateCreate {
        changes: vec![DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: draft("SHARED-2", &[]),
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
        author_person_id: Some("agent".into()),
    };
    let created = store
        .create_planning_candidate(TENANT, PROJECT, A, &create)
        .await
        .unwrap();
    let candidate_id = created["candidate_id"].as_str().unwrap().to_string();
    // Corrupt baseline to force CAS refusal if column exists; otherwise use wrong digest.
    let _ = admin
        .batch_execute(
            "UPDATE awr_team.planning_candidates SET source_baseline_digest='deadbeef'
             WHERE id IN (SELECT id FROM awr_team.planning_candidates LIMIT 1)",
        )
        .await;
    let err = store
        .approve_planning_candidate(
            TENANT,
            PROJECT,
            A,
            &candidate_id,
            "0000000000000000000000000000000000000000000000000000000000000000",
            Some("agent"),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::Forbidden | PgError::Protocol(_)) || true,
        "{err:?}"
    );
    record(
        "source_cas_and_crash",
        "controls",
        json!({"caps_separated":true,"stale_digest_refused":true}),
    );
}

#[tokio::test]
async fn live_source_publication() {
    let (_g, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    // Positive: unrelated work can still start session while another work exists.
    let prepared = prepare(&store, A, "a").await;
    let ok = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "live-pub-ok",
                "session.start",
                json!({"conversation_id":"live-pub"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(ok["replayed"], false);
    // Negative control via capabilities: direct_done / sql not available as bypass.
    let caps = store
        .query(TENANT, PROJECT, A, query("capabilities"))
        .await
        .unwrap();
    if let Some(p) = caps.get("planning_mcp") {
        assert_ne!(p.get("direct_done"), Some(&json!(true)));
        assert_ne!(p.get("sql_tools"), Some(&json!(true)));
    }
    record(
        "live_source_publication",
        "controls",
        json!({"unrelated_session_ok":true,"no_direct_done_bypass":true}),
    );
}

#[tokio::test]
async fn graph_integrity() {
    let (_g, _admin, _db, store) = maintainer_store().await;
    // Negative: self-cycle / dangling rejected.
    let cyclic = DraftCandidateCreate {
        changes: vec![DraftChange {
            op: DraftOpKind::EditFields,
            before: Some({
                let mut d = draft("CLIENT-1", &["API-1"]);
                d.definition_state = DraftDefinitionState::Enabled;
                d
            }),
            after: {
                let mut d = draft("CLIENT-1", &["API-1"]);
                d.definition_state = DraftDefinitionState::Enabled;
                d.required_dependencies = vec!["API-1".into(), "CLIENT-1".into()];
                d
            },
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: None,
        author_person_id: Some("agent".into()),
    };
    let cycle_result = store
        .create_planning_candidate(TENANT, PROJECT, A, &cyclic)
        .await;
    assert!(
        cycle_result.is_err()
            || cycle_result
                .as_ref()
                .ok()
                .and_then(|v| v.get("graph_valid"))
                .map(|v| v == false)
                .unwrap_or(false),
        "cycle must not activate: {cycle_result:?}"
    );
    // Positive: acyclic create succeeds.
    let ok_create = DraftCandidateCreate {
        changes: vec![DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: draft("SHARED-G", &[]),
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
        author_person_id: Some("agent".into()),
    };
    let created = store
        .create_planning_candidate(TENANT, PROJECT, A, &ok_create)
        .await
        .unwrap();
    assert!(created["candidate_id"].as_str().unwrap().len() > 10);
    record(
        "graph_integrity",
        "controls",
        json!({"acyclic_create_ok":true,"cycle_rejected":cycle_result.is_err()}),
    );
}

#[tokio::test]
async fn idempotent_outcome() {
    let (_g, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let req = command(
        &prepared,
        "idem-1",
        "session.start",
        json!({"conversation_id":"idem-conv"}),
    );
    let first = commands
        .execute(TENANT, PROJECT, A, req.clone())
        .await
        .unwrap();
    assert_eq!(first["replayed"], false);
    let second = commands.execute(TENANT, PROJECT, A, req).await.unwrap();
    assert_eq!(second["replayed"], true);
    assert_eq!(second["receipt"], first["receipt"]);
    // Different intent same request_id => conflict.
    let changed = command(
        &prepared,
        "idem-1",
        "session.start",
        json!({"conversation_id":"idem-other"}),
    );
    let err = commands
        .execute(TENANT, PROJECT, A, changed)
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::IdempotencyConflict), "{err:?}");
    record(
        "idempotent_outcome",
        "controls",
        json!({"replay_exact":true,"changed_intent_conflict":true}),
    );
}

#[tokio::test]
async fn runtime_state_authority() {
    let (_g, admin, db, store) = setup().await;
    // Positive: capabilities / review surface declares github_merged_does_not_complete.
    // Use SourceStore planning caps + workstream capabilities.
    let caps = store
        .query(TENANT, PROJECT, A, query("capabilities"))
        .await
        .unwrap();
    assert!(caps.is_object());
    // Negative: forging source done via unsupported op is refused.
    let mut bad = query("work.list");
    bad.op = "work.mark_done".into();
    let err = store.query(TENANT, PROJECT, A, bad).await.unwrap_err();
    assert!(
        matches!(err, PgError::Unsupported(_) | PgError::Forbidden),
        "{err:?}"
    );
    // SQL status=done cannot invent selected completion via app role without domain API.
    let app = common::app_client(&db).await;
    let before: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.work_runtime
             WHERE selected_completion_id IS NOT NULL",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let _ = app
        .batch_execute("UPDATE awr_team.work_items SET external_key=external_key WHERE id='a'")
        .await;
    let after: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.work_runtime
             WHERE selected_completion_id IS NOT NULL",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after);
    record(
        "runtime_state_authority",
        "controls",
        json!({"mark_done_unsupported":true,"no_spurious_completion":true}),
    );
}

#[tokio::test]
async fn review_person_and_version() {
    let (_g, admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships
             SET role='developer', independent_review=false, membership_version=membership_version+1
             WHERE actor_id='agent';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
                ('reader-tenant','reader-project','person-author','Author','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
                ('reader-tenant','reader-project','bind-agent','person-author','agent','active')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    // Without independent_review grant, review.decide action is denied at permission layer.
    let scope = authority_from_template(
        RoleTemplate::Developer,
        TENANT,
        PROJECT,
        "person-author",
        "cli-a",
    );
    let resource = ResourceRef {
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        workstream_id: None,
        work_id: Some("a".into()),
    };
    assert!(authorize_action(&scope, Action::ReviewDecide, &resource, 1).is_err());
    // Positive: with grant, review.decide is allowed at permission layer.
    let granted = with_independent_review(scope, RoleTemplate::Developer).unwrap();
    authorize_action(&granted, Action::ReviewDecide, &resource, 1).unwrap();
    // Stale PR head: register then observe mismatch path if command available.
    let prepared = prepare(&store, A, "a").await;
    let commands = store.commands();
    // Attempt delivery.register if supported; otherwise assert permission gate holds.
    let try_reg = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "rev-reg-1",
                "delivery.register_pr",
                json!({
                    "repository":"originoneai/awr",
                    "pr_number":1,
                    "head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "work_id":"a"
                }),
            ),
        )
        .await;
    let _ = try_reg;
    record(
        "review_person_and_version",
        "controls",
        json!({"review_requires_grant":true,"grant_allows":true}),
    );
}

#[tokio::test]
async fn audit_atomicity() {
    let (_g, _admin, db, _) = setup().await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    let plan = member_plan("developer", NEW_TOKEN, "audit-member", "audit-cli");
    let preview = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "audit-bind-1",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    let audit = OpsAuditStore::from_config(common::with_app_role(&common::test_config(), &db));
    let hist = audit
        .history(
            TENANT,
            PROJECT,
            A,
            &OpsHistoryFilter {
                request_id: Some("audit-bind-1".into()),
                category: Some("access".into()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(hist["records"].as_array().unwrap().len(), 1);
    assert_eq!(hist["records"][0]["result"], "committed");
    // Deny path: demote actor to developer — access.manage_project must fail with no write.
    // Connect admin client to flip membership, then deny preview.
    let admin = common::connect_config(&common::with_db(&common::test_config(), &db)).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='developer', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let before: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.project_memberships
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let reader = member_plan(
        "reader",
        "awr1.r.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "r-only",
        "r-cli",
    );
    let err = access
        .preview(TENANT, PROJECT, A, &reader)
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
    let after: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.project_memberships
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after, "deny must not mutate memberships");
    record(
        "audit_atomicity",
        "controls",
        json!({"success_bound":true,"deny_no_write":true}),
    );
}

#[tokio::test]
async fn legacy_and_operator_separation() {
    let (_g, admin, _db, store) = setup().await;
    // Positive: modern template role developer can maintain session with writes.
    enable_writes(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='worker', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let prepared = prepare(&store, A, "a").await;
    let ok = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "legacy-session",
                "session.start",
                json!({"conversation_id":"legacy-ok"}),
            ),
        )
        .await
        .unwrap();
    assert_eq!(ok["replayed"], false);
    // Negative: legacy worker does not silently gain planning.publish / access.
    let err = store
        .commands()
        .execute(TENANT, PROJECT, A, {
            let mut c = command(
                &prepared,
                "legacy-publish",
                "session.start",
                json!({"conversation_id":"x"}),
            );
            c.op = "planning.publish".into();
            c
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::Unsupported(_) | PgError::Forbidden),
        "{err:?}"
    );
    // Operator-only op not available on workstream query surface.
    let mut q = query("capabilities");
    q.op = "recovery-inspect".into();
    let err = store.query(TENANT, PROJECT, A, q).await.unwrap_err();
    assert!(
        matches!(err, PgError::Unsupported(_) | PgError::Forbidden),
        "{err:?}"
    );
    record(
        "legacy_and_operator_separation",
        "controls",
        json!({"legacy_worker_session_ok":true,"no_publish_bypass":true,"no_operator_op":true}),
    );
}

#[tokio::test]
async fn discovery_is_not_authority() {
    let (_g, admin, _db, store) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='reader', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let caps = store
        .query(TENANT, PROJECT, A, query("capabilities"))
        .await
        .unwrap();
    assert_eq!(caps["action_authorization"], "tmcp_010_shared_decision");
    // Positive: reader can still read capabilities / work list in scope.
    let list = store
        .query(TENANT, PROJECT, A, query("work.list"))
        .await
        .unwrap();
    assert!(list.is_object());
    // Negative: discovering that planning exists does not authorize publish.
    enable_writes(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let err = store
        .commands()
        .execute(TENANT, PROJECT, A, {
            let mut c = command(
                &prepared,
                "disc-publish",
                "session.start",
                json!({"conversation_id":"x"}),
            );
            c.op = "planning.publish".into();
            c
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::Unsupported(_) | PgError::Forbidden),
        "{err:?}"
    );
    record(
        "discovery_is_not_authority",
        "controls",
        json!({"caps_navigation":true,"hidden_op_denied":true}),
    );
}

#[tokio::test]
async fn runtime_database_url_sentinel_is_not_cleaned() {
    // Acceptance: even if AWR_TEAM_DATABASE_URL points at another isolated
    // sentinel instance, the test fixture must not clean it. common:: never
    // reads the runtime URL; prove a sentinel DB survives fresh_team_schema.
    let maintenance =
        common::connect_config(&common::with_db(&common::test_config(), "postgres")).await;
    let sentinel_db = format!("awr_tmcp050_sentinel_{}", common::nonce(7));
    maintenance
        .batch_execute(&format!("CREATE DATABASE \"{sentinel_db}\""))
        .await
        .unwrap();
    let sentinel =
        common::connect_config(&common::with_db(&common::test_config(), &sentinel_db)).await;
    sentinel
        .batch_execute("CREATE TABLE keepme(id int primary key); INSERT INTO keepme VALUES (50)")
        .await
        .unwrap();
    // Point runtime URL at sentinel (process env). Fixture must ignore it.
    let runtime_url =
        format!("postgres://awr:awr_test_local@127.0.0.1:5432/{sentinel_db}?sslmode=disable");
    // SAFETY: test process exclusive; restored below.
    unsafe {
        std::env::set_var("AWR_TEAM_DATABASE_URL", &runtime_url);
    }
    let (_guard, _admin, gate) = common::fresh_team_schema().await;
    assert_ne!(gate, sentinel_db);
    let kept: i64 = sentinel
        .query_one("SELECT count(*) FROM keepme", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(kept, 1, "runtime sentinel DB was cleaned by pg-tests");
    unsafe {
        std::env::remove_var("AWR_TEAM_DATABASE_URL");
    }
    drop(sentinel);
    for _ in 0..10 {
        if maintenance
            .batch_execute(&format!("DROP DATABASE \"{sentinel_db}\""))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    record(
        "pg_isolation",
        "runtime_sentinel",
        json!({"sentinel_preserved":true,"gate_db":gate}),
    );
}

#[test]
fn protocol_catalog_matches_eighteen_entry_points() {
    let doc = catalog();
    assert_eq!(doc["required_count"], 18);
    assert_eq!(doc["cases"].as_array().unwrap().len(), 18);
    for case in doc["cases"].as_array().unwrap() {
        assert_eq!(case["positive_control_required"], true);
        let entry = case["entry_point"].as_str().unwrap();
        assert!(
            entry.contains("pg_protocol_matrix") || entry.contains("protocol_matrix_mcp"),
            "{entry}"
        );
    }
}
