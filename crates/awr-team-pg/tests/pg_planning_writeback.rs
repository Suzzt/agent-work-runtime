//! AWR-TMCP-022: planning writeback activation gate and receipts on real PG.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team::{
    DraftChange, DraftDefinitionState, DraftOpKind, OrdinaryPlanningSelfApprovePolicy, TaskDraft,
};
use awr_team_pg::{DraftCandidateCreate, PgError, SourceStore, WritebackActivateRequest};
use fixture::*;
use std::path::PathBuf;

fn draft(id: &str, deps: &[&str], state: DraftDefinitionState) -> TaskDraft {
    TaskDraft {
        work_id: id.into(),
        external_key: id.into(),
        title: format!("Task {id}"),
        goals: vec!["delivery".into()],
        scope_paths: vec!["specs/api.md".into()],
        acceptance: vec!["ok".into()],
        required_dependencies: deps.iter().map(|s| (*s).into()).collect(),
        completion_policy: "independent_review".into(),
        definition_state: state,
        split_from: None,
        split_children: vec![],
    }
}

async fn store_and_roles() -> (
    std::sync::MutexGuard<'static, ()>,
    tokio_postgres::Client,
    String,
    SourceStore,
) {
    let (guard, admin, db, _read) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='maintainer', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
               ('reader-tenant','reader-project','API-1','API-1'),
               ('reader-tenant','reader-project','CLIENT-1','CLIENT-1'),
               ('reader-tenant','reader-project','OTHER-1','OTHER-1')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    (guard, admin, db, store)
}

async fn publish_candidate(store: &SourceStore) -> (String, String, String) {
    let create = DraftCandidateCreate {
        changes: vec![
            DraftChange {
                op: DraftOpKind::CreateTask,
                before: None,
                after: draft("SHARED-1", &[], DraftDefinitionState::Draft),
            },
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft("CLIENT-1", &["API-1"], DraftDefinitionState::Enabled)),
                after: draft(
                    "CLIENT-1",
                    &["API-1", "SHARED-1"],
                    DraftDefinitionState::Enabled,
                ),
            },
        ],
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
    store
        .approve_planning_candidate(TENANT, PROJECT, A, &candidate_id, &digest, Some("agent"))
        .await
        .unwrap();
    let published = store
        .publish_planning_candidate(TENANT, PROJECT, A, &candidate_id, &digest)
        .await
        .unwrap();
    let receipt_id = published["receipt_id"].as_str().unwrap().to_string();
    (candidate_id, digest, receipt_id)
}

#[tokio::test]
async fn writeback_capabilities_and_runtime_fields_separated() {
    let caps = SourceStore::planning_writeback_capabilities();
    assert_eq!(caps["source_writeback"], "tmcp_022");
    assert_eq!(caps["project_claim_barrier_retained_when_unproven"], true);
    assert_eq!(
        caps["cancel_expiry_session_end_prove_process_stopped"],
        false
    );
    assert_eq!(caps["runtime_fields_writable_via_source"], false);
    assert_eq!(caps["source_status_is_completion_receipt"], false);
    assert_eq!(caps["idempotent_request_id"], true);
    let planning = SourceStore::planning_capabilities();
    assert_eq!(planning["source_writeback"], "tmcp_022");
}

#[tokio::test]
async fn unproven_impact_conservatively_refuses_with_recovery_actions() {
    let (_g, _admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate(&store).await;
    let tmp = tempfile_ledger();
    let err = store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &WritebackActivateRequest {
                request_id: "req-unproven-1".into(),
                publish_receipt_id: receipt_id,
                source_root: tmp.root.clone(),
                ledger_relative_path: "ledger.yaml".into(),
                impact_proven: false,
                stopped_work_ids: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::ActivationImpactUnproven(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn affected_live_claim_requires_explicit_stop() {
    let (_g, admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate(&store).await;
    // Plant an active claim on CLIENT-1 (affected).
    admin
        .batch_execute(
            "INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,last_fence)
             VALUES ('reader-tenant','reader-project','main','CLIENT-1','claimed',0)
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.sessions(
                tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state)
             VALUES ('reader-tenant','reader-project','sess-wb','main','CLIENT-1','agent','cli-a','c','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.claims(
                tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,fence,state,expires_at)
             VALUES (
                'reader-tenant','reader-project','claim-wb','main','CLIENT-1','sess-wb','agent',1,
                'active', clock_timestamp() + interval '1 hour')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let tmp = tempfile_ledger();
    let err = store
        .activate_planning_writeback(
            TENANT,
            PROJECT,
            A,
            &WritebackActivateRequest {
                request_id: "req-live-claim-1".into(),
                publish_receipt_id: receipt_id,
                source_root: tmp.root.clone(),
                ledger_relative_path: "ledger.yaml".into(),
                impact_proven: true,
                stopped_work_ids: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::WritebackRefused(_)), "{err:?}");
}

struct TmpLedger {
    root: PathBuf,
}

fn tempfile_ledger() -> TmpLedger {
    let root = std::env::temp_dir().join(format!(
        "awr-tmcp022-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let ledger = r#"workstreams:
  version: 1
  definitions:
    - id: 01K00000000000000000000001
      external_key: api
      title: API
      state: active
      authority_version: 1
      goal_keys: [delivery]
      acceptance_contracts: []
    - id: 01K00000000000000000000002
      external_key: client
      title: Client
      state: active
      authority_version: 1
      goal_keys: [delivery]
      acceptance_contracts: []
goals:
  - id: delivery
    title: Deliver
    status: active
work_items:
  - id: API-1
    title: Define
    status: completed
    workstream: api
    goals: [delivery]
    acceptance: [OpenAPI is reviewed]
    paths: [openapi.yaml]
    depends_on: []
  - id: CLIENT-1
    title: SDK
    status: planned
    workstream: client
    goals: [delivery]
    acceptance: [SDK smoke test passes]
    paths: [sdk/]
    depends_on: [API-1]
  - id: OTHER-1
    title: Unrelated
    status: planned
    workstream: client
    goals: [delivery]
    acceptance: [ok]
    paths: [other/]
    depends_on: []
"#;
    std::fs::write(root.join("ledger.yaml"), ledger).unwrap();
    TmpLedger { root }
}

#[tokio::test]
async fn refused_writeback_journal_is_durable_and_replay_stays_refused() {
    let (_g, _admin, _db, store) = store_and_roles().await;
    let (_cid, _digest, receipt_id) = publish_candidate(&store).await;
    let tmp = tempfile_ledger();
    let req = WritebackActivateRequest {
        request_id: "req-replay-1".into(),
        publish_receipt_id: receipt_id,
        source_root: tmp.root.clone(),
        ledger_relative_path: "ledger.yaml".into(),
        impact_proven: false,
        stopped_work_ids: vec![],
    };
    let err1 = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap_err();
    assert!(
        matches!(err1, PgError::ActivationImpactUnproven(_)),
        "{err1:?}"
    );
    let err2 = store
        .activate_planning_writeback(TENANT, PROJECT, A, &req)
        .await
        .unwrap_err();
    assert!(
        matches!(err2, PgError::ActivationImpactUnproven(_)),
        "{err2:?}"
    );
    // No activation receipt until success.
    let receipt = store
        .get_planning_activation_receipt(TENANT, PROJECT, A, "req-replay-1")
        .await
        .unwrap();
    assert!(receipt.is_none());
}
