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
    let (_g, _admin, db, _store) = setup().await;
    let access = ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
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
    let (_g, _admin, db, store) = setup().await;
    let access = ProjectAccessStore::from_config(common::with_app_role(&common::test_config(), &db));
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
    let hist = store
        .query(TENANT, PROJECT, READER_TOKEN, q)
        .await
        .unwrap();
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
    assert_eq!(out["protocol"], "awr-planning-command-receipt-v1");
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
