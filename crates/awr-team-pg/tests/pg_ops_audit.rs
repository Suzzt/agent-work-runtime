#![cfg(feature = "pg-tests")]
//! AWR-TMCP-040: ops audit atomicity, deny capacity, scoped history/export.

mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team_pg::{
    AdminAccessPlan, DENY_CAPACITY_PER_PROJECT, OpsAuditStore, OpsCategory, OpsDenyWrite,
    OpsHistoryFilter, PgError, ProjectAccessStore, SourceStore, digest_of, record_deny,
    workstream_credential_hash,
};
use fixture::*;
use serde_json::json;
use tokio_postgres::Client;

const NEW_TOKEN: &str =
    "awr1.new-member.eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const READER_TOKEN: &str =
    "awr1.reader-only.ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

fn admin_plan_member() -> AdminAccessPlan {
    serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"new-human","kind":"human","display_name":"New member"},
        "subject_client_id":"new-cli",
        "role":"developer",
        "grants":[{
            "workstream_id":awr_core::Id::from(1),
            "authority_version":"1",
            "read":true,"write":true,"manage":false,
            "attest_execution":false,"reconcile_execution":false
        }],
        "credential":{
            "id":"new-member",
            "secret_hash":workstream_credential_hash(NEW_TOKEN).unwrap(),
            "expires_at_unix_ms":null
        },
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap()
}

async fn enable_admin_manage(owner: &Client) {
    owner
        .batch_execute(
            "UPDATE awr_team.workstream_grants
             SET can_write=true, can_manage=true, grant_version=grant_version+1
             WHERE client_id='cli-a'",
        )
        .await
        .unwrap();
}

fn reader_plan() -> AdminAccessPlan {
    serde_json::from_value(json!({
        "protocol_version":1,
        "subject":{"id":"reader-human","kind":"human","display_name":"Reader"},
        "subject_client_id":"reader-cli",
        "role":"reader",
        "grants":[{
            "workstream_id":awr_core::Id::from(1),
            "authority_version":"1",
            "read":true,"write":false,"manage":false,
            "attest_execution":false,"reconcile_execution":false
        }],
        "credential":{
            "id":"reader-only",
            "secret_hash":workstream_credential_hash(READER_TOKEN).unwrap(),
            "expires_at_unix_ms":null
        },
        "remove_membership":false,
        "revoke_tenant_credentials":[]
    }))
    .unwrap()
}

#[tokio::test]
async fn access_apply_binds_ops_audit_same_tx_and_export_authorized() {
    let (_g, admin, db, _store) = setup().await;
    enable_admin_manage(&admin).await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    let plan = admin_plan_member();
    let preview = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    let applied = access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "add-member-audit",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(applied["replayed"], false);

    let audit = OpsAuditStore::from_config(common::with_app_role(&common::test_config(), &db));
    let hist = audit
        .history(
            TENANT,
            PROJECT,
            A,
            &OpsHistoryFilter {
                request_id: Some("add-member-audit".into()),
                category: Some("access".into()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(hist["scope"], "project");
    let records = hist["records"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["action"], "access.manage_project");
    assert_eq!(records[0]["request_id"], "add-member-audit");
    assert_eq!(records[0]["result"], "committed");
    assert_eq!(hist["non_repudiation"], "not_claimed_against_db_owner");
    assert_eq!(hist["chat_text_collected"], false);

    let export = audit
        .export(
            TENANT,
            PROJECT,
            A,
            &OpsHistoryFilter {
                request_id: Some("add-member-audit".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(export["export"], true);
    assert_eq!(export["token_billing_collected"], false);
}

#[tokio::test]
async fn deny_is_capacity_bounded_redacted_and_non_mutating() {
    let (_g, admin, db, _store) = setup().await;
    let cfg = common::with_app_role(&common::test_config(), &db);
    let pool = awr_team_pg::PgPool::from_config(cfg);
    let before: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.project_memberships
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);

    let n = DENY_CAPACITY_PER_PROJECT + 20;
    for i in 0..n {
        record_deny(
            &pool,
            TENANT,
            PROJECT,
            &OpsDenyWrite {
                category: OpsCategory::Access,
                action: "access.manage_project".into(),
                actor_id: Some("agent".into()),
                client_id: Some("cli-a".into()),
                person_id: None,
                target_kind: Some("member".into()),
                target_id: Some(format!("subj-{i}")),
                request_id: Some(format!("deny-{i}")),
                reason_code: "permission_denied".into(),
            },
        )
        .await
        .unwrap();
    }
    let kept: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.ops_audit_denies
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert!(kept <= DENY_CAPACITY_PER_PROJECT);
    // Fresh deny after prune so redaction is observable (oldest rows were pruned).
    record_deny(
        &pool,
        TENANT,
        PROJECT,
        &OpsDenyWrite {
            category: OpsCategory::Access,
            action: "access.manage_project".into(),
            actor_id: Some("agent".into()),
            client_id: Some("cli-a".into()),
            person_id: None,
            target_kind: Some("member".into()),
            target_id: Some("subj-redact".into()),
            request_id: Some("deny-redact".into()),
            reason_code: "token exposed".into(),
        },
    )
    .await
    .unwrap();
    let redacted: String = admin
        .query_one(
            "SELECT reason_code FROM awr_team.ops_audit_denies
             WHERE tenant_id=$1 AND project_id=$2 AND request_id='deny-redact'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(redacted, "redacted_deny");

    let after: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.project_memberships
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after);
}

#[tokio::test]
async fn member_history_count_cannot_cross_scope() {
    let (_g, admin, db, store) = setup().await;
    enable_admin_manage(&admin).await;
    let access =
        ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
    // Add a reader member via admin.
    let plan = reader_plan();
    let preview = access.preview(TENANT, PROJECT, A, &plan).await.unwrap();
    access
        .apply(
            TENANT,
            PROJECT,
            A,
            &plan,
            "add-reader-audit",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();

    let pool = awr_team_pg::PgPool::from_config(common::with_app_role(&common::test_config(), &db));
    record_deny(
        &pool,
        TENANT,
        PROJECT,
        &OpsDenyWrite {
            category: OpsCategory::Planning,
            action: "planning.publish".into(),
            actor_id: Some("agent".into()),
            client_id: Some("cli-a".into()),
            person_id: None,
            target_kind: Some("candidate".into()),
            target_id: Some("c1".into()),
            request_id: Some("admin-deny".into()),
            reason_code: "permission_denied".into(),
        },
    )
    .await
    .unwrap();
    record_deny(
        &pool,
        TENANT,
        PROJECT,
        &OpsDenyWrite {
            category: OpsCategory::Planning,
            action: "planning.propose".into(),
            actor_id: Some("reader-human".into()),
            client_id: Some("reader-cli".into()),
            person_id: None,
            target_kind: Some("suggestion".into()),
            target_id: Some("s1".into()),
            request_id: Some("reader-deny".into()),
            reason_code: "permission_denied".into(),
        },
    )
    .await
    .unwrap();

    let mut q = query("audit.history");
    q.include_denies = Some(true);
    q.limit = Some(50);
    let hist = store.query(TENANT, PROJECT, READER_TOKEN, q).await.unwrap();
    assert_eq!(hist["scope"], "self");
    let denies = hist["denies"].as_array().unwrap();
    assert!(denies.iter().all(|d| d["actor_id"] == "reader-human"));
    assert!(!denies.iter().any(|d| d["request_id"] == "admin-deny"));

    let mut q = query("audit.count");
    q.include_denies = Some(true);
    q.member_actor_id = Some("agent".into());
    let cross = store.query(TENANT, PROJECT, READER_TOKEN, q).await;
    assert!(matches!(cross, Err(PgError::Forbidden)));
}

#[tokio::test]
async fn planning_suggest_binds_receipt_with_ops_audit() {
    let (_g, admin, db, _read) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='developer', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
               ('reader-tenant','reader-project','API-1','API-1')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let source = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let req = awr_team_pg::PlanningSuggestRequest {
        protocol_version: 1,
        request_id: "plan-audit-1".into(),
        rationale: "need shared types".into(),
        affected_work_keys: vec!["API-1".into()],
        proposed_notes: json!({"add":"SHARED"}),
        author_person_id: Some("agent".into()),
    };
    let out = source
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(out["protocol"], "awr-team-planning-command-v1");
    assert_eq!(out["request_id"], "plan-audit-1");
    assert_eq!(out["already_recorded"], false);

    // Promote to project_admin so audit.read_project works for history.
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='project_admin', membership_version=membership_version+1
             WHERE actor_id='agent'",
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
                request_id: Some("plan-audit-1".into()),
                category: Some("planning".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let records = hist["records"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["action"], "planning.propose");
    assert_eq!(records[0]["actor_id"], "agent");
}

#[test]
fn digest_helper_stable() {
    assert_eq!(digest_of(&json!({"x":1})).len(), 64);
    assert_eq!(digest_of(&json!({"x":1})), digest_of(&json!({"x":1})));
}

const HEAD_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

async fn enable_delivery_writes(admin: &Client) {
    enable_writes(admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.workstream_grants
             SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
                ('reader-tenant','reader-project','person-author','Author','active')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn mcp_delivery_register_pr_binds_ops_audit_by_request_id() {
    let (_g, admin, db, store) = setup().await;
    enable_delivery_writes(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let out = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "mcp-reg-pr-audit",
                "delivery.register_pr",
                json!({
                    "session_id":"session-a",
                    "expected_session_version":"1",
                    "repository":"originoneai/awr",
                    "pr_number":134,
                    "pr_url":"https://github.com/originoneai/awr/pull/134",
                    "head_sha": HEAD_SHA,
                    "fact_source":"authorized_human_github_verification",
                    "observed_at":"2026-09-23T04:00:00+08:00",
                    "author_actor_id":"agent",
                    "owner_person_id":"person-author",
                    "executor_actor_id":"agent",
                    "gh_submitted": true
                }),
            ),
        )
        .await
        .unwrap();
    assert_eq!(out["replayed"], false);
    assert_eq!(out["receipt"]["data"]["state"], "active");

    let audit = OpsAuditStore::from_config(common::with_app_role(&common::test_config(), &db));
    let hist = audit
        .history(
            TENANT,
            PROJECT,
            A,
            &OpsHistoryFilter {
                request_id: Some("mcp-reg-pr-audit".into()),
                category: Some("delivery".into()),
                limit: Some(10),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let records = hist["records"].as_array().unwrap();
    assert_eq!(records.len(), 1, "{hist}");
    assert_eq!(records[0]["action"], "delivery.register_pr");
    assert_eq!(records[0]["request_id"], "mcp-reg-pr-audit");
    assert_eq!(records[0]["actor_id"], "agent");
    assert_eq!(records[0]["client_id"], "cli-a");
    assert_eq!(records[0]["result"], "committed");
    assert_eq!(records[0]["work_id"], "a");
    assert_eq!(
        records[0]["target_id"],
        out["receipt"]["data"]["delivery_id"]
    );
}

#[tokio::test]
async fn mcp_delivery_ops_audit_abort_rolls_back_domain_receipt_and_audit() {
    let (_g, admin, db, store) = setup().await;
    enable_delivery_writes(&admin).await;
    let before_ops: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.operations
             WHERE tenant_id=$1 AND project_id=$2 AND request_id='inject-ops-audit-abort'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let before_pr: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.pr_deliveries
             WHERE tenant_id=$1 AND project_id=$2 AND work_id='a'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let before_audit: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.ops_audit_records
             WHERE tenant_id=$1 AND project_id=$2 AND request_id='inject-ops-audit-abort'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);

    let prepared = prepare(&store, A, "a").await;
    let err = store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "inject-ops-audit-abort",
                "delivery.register_pr",
                json!({
                    "session_id":"session-a",
                    "expected_session_version":"1",
                    "repository":"originoneai/awr",
                    "pr_number":999,
                    "pr_url":"https://github.com/originoneai/awr/pull/999",
                    "head_sha": HEAD_SHA,
                    "fact_source":"operator_recorded_observation",
                    "observed_at":"2026-09-23T04:05:00+08:00",
                    "gh_submitted": true
                }),
            ),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::Protocol(ref m) if m == "injected ops audit abort"),
        "{err:?}"
    );

    let after_ops: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.operations
             WHERE tenant_id=$1 AND project_id=$2 AND request_id='inject-ops-audit-abort'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let after_pr: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.pr_deliveries
             WHERE tenant_id=$1 AND project_id=$2 AND work_id='a'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let after_audit: i64 = admin
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.ops_audit_records
             WHERE tenant_id=$1 AND project_id=$2 AND request_id='inject-ops-audit-abort'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(before_ops, after_ops);
    assert_eq!(before_pr, after_pr);
    assert_eq!(before_audit, after_audit);

    let audit = OpsAuditStore::from_config(common::with_app_role(&common::test_config(), &db));
    let hist = audit
        .history(
            TENANT,
            PROJECT,
            A,
            &OpsHistoryFilter {
                request_id: Some("inject-ops-audit-abort".into()),
                category: Some("delivery".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(hist["records"].as_array().unwrap().is_empty());
}
