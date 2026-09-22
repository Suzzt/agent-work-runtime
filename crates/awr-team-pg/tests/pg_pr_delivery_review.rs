#![cfg(feature = "pg-tests")]
//! AWR-TMCP-031: PR delivery binding + independent review.decide / delivery.finalize.

mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team_pg::{PgError, workstream_credential_hash};
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

const REVIEWER_TOKEN: &str =
    "awr1.reviewer-h.eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEAD2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MERGE: &str = "cccccccccccccccccccccccccccccccccccccccc";

async fn grant_independent_reviewer(admin: &Client) {
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships
             SET role='developer', independent_review=true,
                 membership_version=membership_version+1
             WHERE actor_id='reviewer';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
                ('reader-tenant','reader-project','person-author','Author','active'),
                ('reader-tenant','reader-project','reviewer','Reviewer','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
                ('reader-tenant','reader-project','bind-agent','person-author','agent','active')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let hash = workstream_credential_hash(REVIEWER_TOKEN).unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
             VALUES($1,'reviewer-h','reviewer','cli-reviewer',$2) ON CONFLICT DO NOTHING",
            &[&TENANT, &hash],
        )
        .await
        .unwrap();
    let stream: String = admin
        .query_one(
            "SELECT workstream_id FROM awr_team.sessions WHERE id='session-a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    admin
        .execute(
            "INSERT INTO awr_team.workstream_grants(
                tenant_id,project_id,actor_id,client_id,workstream_id,
                authority_version,can_read,can_write)
             VALUES($1,$2,'reviewer','cli-reviewer',$3,1,true,true)
             ON CONFLICT DO NOTHING",
            &[&TENANT, &PROJECT, &stream],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.sessions(
                tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,
                conversation_id,state,workstream_id,ownership_version)
             VALUES($1,$2,'session-reviewer','main','a','reviewer','cli-reviewer',
                    'session-reviewer','active',$3,1)
             ON CONFLICT DO NOTHING",
            &[&TENANT, &PROJECT, &stream],
        )
        .await
        .unwrap();
    enable_writes(admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants
             SET can_write=true, grant_version=grant_version+1
             WHERE client_id IN ('cli-a','cli-reviewer')",
        )
        .await
        .unwrap();
}

async fn run(
    store: &awr_team_pg::WorkstreamReadStore,
    token: &str,
    request: &str,
    op: &str,
    args: Value,
) -> Value {
    let prepared = prepare(store, token, "a").await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(&prepared, request, op, args),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone()
}

async fn run_err(
    store: &awr_team_pg::WorkstreamReadStore,
    token: &str,
    request: &str,
    op: &str,
    args: Value,
) -> PgError {
    let prepared = prepare(store, token, "a").await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(&prepared, request, op, args),
        )
        .await
        .unwrap_err()
}

#[tokio::test]
async fn independent_review_grant_required_for_decide() {
    let (_g, admin, _, store) = setup().await;
    grant_independent_reviewer(&admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships
             SET independent_review=false, membership_version=membership_version+1
             WHERE actor_id='reviewer'",
        )
        .await
        .unwrap();
    let err = run_err(
        &store,
        REVIEWER_TOKEN,
        "deny-decide",
        "review.decide",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id":"missing",
            "decision":"approve",
            "reason":"nope"
        }),
    )
    .await;
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
}

#[tokio::test]
async fn pr_register_observe_and_status_separate_from_acceptance() {
    let (_g, admin, _, store) = setup().await;
    grant_independent_reviewer(&admin).await;

    let registered = run(
        &store,
        A,
        "reg-pr-1",
        "delivery.register_pr",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "repository":"originoneai/awr",
            "pr_number":131,
            "pr_url":"https://github.com/originoneai/awr/pull/131",
            "head_sha": HEAD,
            "fact_source":"authorized_human_github_verification",
            "observed_at":"2026-09-23T02:00:00Z",
            "author_actor_id":"agent",
            "owner_person_id":"person-author",
            "executor_actor_id":"agent",
            "gh_submitted": true,
            "gh_approved": false,
            "gh_merged": false
        }),
    )
    .await;
    assert_eq!(registered["state"], "active");
    assert_eq!(registered["awr_acceptance_complete"], false);
    assert_eq!(registered["webhook_auto_sync"], false);
    let delivery_id = registered["delivery_id"].as_str().unwrap().to_string();

    let observed = run(
        &store,
        A,
        "obs-pr-1",
        "delivery.observe_pr",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "delivery_id": delivery_id,
            "expected_head_sha": HEAD,
            "fact_source":"authorized_human_github_verification",
            "observed_at":"2026-09-23T02:10:00Z",
            "gh_approved": true,
            "gh_merged": true,
            "merge_sha": MERGE
        }),
    )
    .await;
    assert_eq!(observed["gh_merged"], true);
    assert_eq!(observed["awr_acceptance_complete"], false);

    let invalidated = run(
        &store,
        A,
        "obs-pr-bad-head",
        "delivery.observe_pr",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "delivery_id": delivery_id,
            "expected_head_sha": HEAD2,
            "fact_source":"authorized_human_github_verification",
            "observed_at":"2026-09-23T02:20:00Z"
        }),
    )
    .await;
    assert_eq!(invalidated["state"], "invalidated");
    assert_eq!(invalidated["invalidation_reason"], "head_sha_mismatch");

    let mut q = query("delivery.inspect");
    q.work_id = Some("a".into());
    let data = store.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone();
    // Active delivery cleared after head mismatch invalidation.
    assert!(data["delivery"]["pr"].is_null());
    assert_eq!(data["delivery"]["awr_acceptance"]["complete"], false);
    assert_eq!(data["delivery"]["webhook_auto_sync"], false);
}

#[tokio::test]
async fn review_decide_denied_for_admin_without_independent_grant() {
    let (_g, admin, _, store) = setup().await;
    grant_independent_reviewer(&admin).await;
    let err = run_err(
        &store,
        A,
        "admin-decide",
        "review.decide",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "round_id":"x",
            "decision":"approve",
            "reason":"admin cannot skip"
        }),
    )
    .await;
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
}

#[tokio::test]
async fn delivery_inspect_query_separates_surfaces() {
    let (_g, admin, _, store) = setup().await;
    grant_independent_reviewer(&admin).await;
    run(
        &store,
        A,
        "reg-pr-2",
        "delivery.register_pr",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "repository":"originoneai/awr",
            "pr_number":99,
            "pr_url":"https://github.com/originoneai/awr/pull/99",
            "head_sha": HEAD,
            "merge_sha": MERGE,
            "fact_source":"operator_recorded_observation",
            "observed_at":"2026-09-23T03:00:00+08:00",
            "gh_submitted": true,
            "gh_approved": true,
            "gh_merged": true
        }),
    )
    .await;
    let mut q = query("delivery.inspect");
    q.work_id = Some("a".into());
    let data = store.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone();
    assert_eq!(data["delivery"]["github"]["merged"], true);
    assert_eq!(data["delivery"]["awr_acceptance"]["complete"], false);
    assert_eq!(data["delivery"]["webhook_auto_sync"], false);
    assert!(
        data["delivery"]["cannot_skip_acceptance_via"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "already_merged")
    );
}
