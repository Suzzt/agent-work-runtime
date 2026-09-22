#![cfg(feature = "pg-tests")]
mod common;
use awr_team_pg::{
    AdoptedConsumerEdge, BoundaryDecision, BoundaryRevalidateRequest, BoundarySnapshot,
    CancelSplitRelation, DecidePlanningChangeRequest, ExecutionBoundary, ProviderChangeKind,
    RecordPlanningChangeRequest, SelectiveInvalidateRequest, SelectiveInvalidationStore,
};
use common::{fresh_team_schema, test_config, with_app_role};
use std::sync::MutexGuard;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (
    MutexGuard<'static, ()>,
    SelectiveInvalidationStore,
    tokio_postgres::Client,
) {
    let (guard, admin, db) = fresh_team_schema().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
                  ('tenant-a','project-a','api','API'),
                  ('tenant-a','project-a','sdk','SDK'),
                  ('tenant-a','project-a','integration','INT'),
                  ('tenant-a','project-a','docs','DOCS');
             INSERT INTO awr_team.dependency_bindings(
                tenant_id,project_id,downstream_work_id,upstream_work_id,binding_hash,valid)
             VALUES
               ('tenant-a','project-a','sdk','api','bind-sdk',true),
               ('tenant-a','project-a','integration','api','bind-int',true);",
        )
        .await
        .unwrap();
    let cfg = with_app_role(&test_config(), &db);
    (guard, SelectiveInvalidationStore::from_config(cfg), admin)
}

fn consumers() -> Vec<AdoptedConsumerEdge> {
    vec![
        AdoptedConsumerEdge {
            dependency_id: "dep-fixed".into(),
            consumer_work_id: "sdk".into(),
            provider_work_id: "api".into(),
            policy: "fixed_delivery".into(),
            credential_status: "active".into(),
            assessment_status: "satisfied".into(),
        },
        AdoptedConsumerEdge {
            dependency_id: "dep-current".into(),
            consumer_work_id: "integration".into(),
            provider_work_id: "api".into(),
            policy: "current_contract".into(),
            credential_status: "active".into(),
            assessment_status: "satisfied".into(),
        },
    ]
}

#[tokio::test]
async fn fixed_consumer_survives_upstream_progress_current_is_invalidated() {
    let (_g, store, admin) = setup().await;
    let (plan, receipt) = store
        .apply_selective_invalidation(
            TENANT,
            PROJECT,
            &SelectiveInvalidateRequest {
                request_key: "inv-1".into(),
                event_id: "evt-1".into(),
                provider_work_id: "api".into(),
                change: ProviderChangeKind::NewVersionOrProgress,
                consumers: consumers(),
                all_project_work_ids: vec![
                    "api".into(),
                    "sdk".into(),
                    "integration".into(),
                    "docs".into(),
                ],
                now_ms: 100,
            },
        )
        .await
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(plan.reevaluate, vec!["integration".to_string()]);
    assert_eq!(plan.leave_valid, vec!["sdk".to_string()]);
    assert_eq!(plan.unaffected_work_ids, vec!["docs".to_string()]);

    // Bindings: sdk stays valid; integration invalidated.
    let sdk_valid: bool = admin
        .query_one(
            "SELECT valid FROM awr_team.dependency_bindings
             WHERE tenant_id='tenant-a' AND project_id='project-a'
               AND downstream_work_id='sdk' AND upstream_work_id='api'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    let int_valid: bool = admin
        .query_one(
            "SELECT valid FROM awr_team.dependency_bindings
             WHERE tenant_id='tenant-a' AND project_id='project-a'
               AND downstream_work_id='integration' AND upstream_work_id='api'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(sdk_valid);
    assert!(!int_valid);

    // Idempotent replay.
    let (plan2, receipt2) = store
        .apply_selective_invalidation(
            TENANT,
            PROJECT,
            &SelectiveInvalidateRequest {
                request_key: "inv-1".into(),
                event_id: "evt-1".into(),
                provider_work_id: "api".into(),
                change: ProviderChangeKind::NewVersionOrProgress,
                consumers: consumers(),
                all_project_work_ids: vec![
                    "api".into(),
                    "sdk".into(),
                    "integration".into(),
                    "docs".into(),
                ],
                now_ms: 100,
            },
        )
        .await
        .unwrap();
    assert!(receipt2.replayed);
    assert_eq!(plan2.reevaluate, plan.reevaluate);
}

#[tokio::test]
async fn boundary_revalidation_blocks_revoke_race_and_keeps_mid_execution_effects() {
    let (_g, store, _) = setup().await;
    // Invalidate integration binding first.
    store
        .apply_selective_invalidation(
            TENANT,
            PROJECT,
            &SelectiveInvalidateRequest {
                request_key: "inv-2".into(),
                event_id: "evt-2".into(),
                provider_work_id: "api".into(),
                change: ProviderChangeKind::DeliveryRevoked,
                consumers: consumers(),
                all_project_work_ids: vec![
                    "api".into(),
                    "sdk".into(),
                    "integration".into(),
                    "docs".into(),
                ],
                now_ms: 110,
            },
        )
        .await
        .unwrap();

    let (prep, _) = store
        .revalidate_boundary(
            TENANT,
            PROJECT,
            &BoundaryRevalidateRequest {
                request_key: "br-prep".into(),
                check_id: "chk-prep".into(),
                snapshot: BoundarySnapshot {
                    boundary: ExecutionBoundary::Prepare,
                    work_id: "sdk".into(),
                    execution_id: None,
                    dependencies_satisfied: true, // caller thought ok; lock recheck flips it
                    revoke_or_invalidation_observed: false,
                    has_real_effects: false,
                    unrelated_work_ids: vec!["docs".into()],
                },
                now_ms: 120,
            },
        )
        .await
        .unwrap();
    assert_eq!(prep.decision, BoundaryDecision::BlockNewEffects);
    assert!(!prep.erase_effects);
    assert!(prep.unrelated_may_continue);

    let (mid, _) = store
        .revalidate_boundary(
            TENANT,
            PROJECT,
            &BoundaryRevalidateRequest {
                request_key: "br-mid".into(),
                check_id: "chk-mid".into(),
                snapshot: BoundarySnapshot {
                    boundary: ExecutionBoundary::Dispatch,
                    work_id: "sdk".into(),
                    execution_id: Some("ex-1".into()),
                    dependencies_satisfied: false,
                    revoke_or_invalidation_observed: true,
                    has_real_effects: true,
                    unrelated_work_ids: vec!["docs".into()],
                },
                now_ms: 130,
            },
        )
        .await
        .unwrap();
    assert_eq!(mid.decision, BoundaryDecision::KeepEffectsAssignRecovery);
    assert!(mid.recovery_duty);
    assert!(!mid.erase_effects);
}

#[tokio::test]
async fn discovered_dependency_blocks_affected_until_confirmed() {
    let (_g, store, _) = setup().await;
    let (change, unrelated, receipt) = store
        .record_planning_change(
            TENANT,
            PROJECT,
            &RecordPlanningChangeRequest {
                request_key: "pc-rec".into(),
                change_id: "pc-1".into(),
                discovered_by: "agent-a".into(),
                old_graph_version: "g1".into(),
                new_graph_version: "g2".into(),
                old_acceptance_contract: "c1".into(),
                new_acceptance_contract: "c2".into(),
                affected_work_ids: vec!["sdk".into(), "integration".into()],
                cancel_split_relations: vec![CancelSplitRelation {
                    kind: "split".into(),
                    from_work_id: "sdk".into(),
                    to_work_id: "sdk-auth".into(),
                }],
                continue_conditions: vec!["export-grant-present".into()],
                all_project_work_ids: vec![
                    "api".into(),
                    "sdk".into(),
                    "integration".into(),
                    "docs".into(),
                ],
                now_ms: 200,
            },
        )
        .await
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(change.status, "affected_blocked");
    assert_eq!(unrelated, vec!["api".to_string(), "docs".to_string()]);
    assert_eq!(
        store.action_blocked(TENANT, PROJECT, "sdk").await.unwrap(),
        Some("pc-1".into())
    );
    assert_eq!(
        store.action_blocked(TENANT, PROJECT, "docs").await.unwrap(),
        None
    );

    // Prepare on blocked work fails admission via boundary recheck.
    let (blocked, _) = store
        .revalidate_boundary(
            TENANT,
            PROJECT,
            &BoundaryRevalidateRequest {
                request_key: "br-blocked".into(),
                check_id: "chk-blocked".into(),
                snapshot: BoundarySnapshot {
                    boundary: ExecutionBoundary::Prepare,
                    work_id: "sdk".into(),
                    execution_id: None,
                    dependencies_satisfied: true,
                    revoke_or_invalidation_observed: false,
                    has_real_effects: false,
                    unrelated_work_ids: vec!["docs".into()],
                },
                now_ms: 210,
            },
        )
        .await
        .unwrap();
    assert_eq!(blocked.decision, BoundaryDecision::BlockNewEffects);

    // Docs (unrelated) may still pass.
    let (docs_ok, _) = store
        .revalidate_boundary(
            TENANT,
            PROJECT,
            &BoundaryRevalidateRequest {
                request_key: "br-docs".into(),
                check_id: "chk-docs".into(),
                snapshot: BoundarySnapshot {
                    boundary: ExecutionBoundary::Prepare,
                    work_id: "docs".into(),
                    execution_id: None,
                    dependencies_satisfied: true,
                    revoke_or_invalidation_observed: false,
                    has_real_effects: false,
                    unrelated_work_ids: vec![],
                },
                now_ms: 220,
            },
        )
        .await
        .unwrap();
    assert_eq!(docs_ok.decision, BoundaryDecision::Allow);

    let (confirmed, _) = store
        .confirm_planning_change(
            TENANT,
            PROJECT,
            &DecidePlanningChangeRequest {
                request_key: "pc-confirm".into(),
                change_id: "pc-1".into(),
                actor_id: "owner".into(),
                now_ms: 230,
            },
        )
        .await
        .unwrap();
    assert_eq!(confirmed.status, "confirmed");
    assert_eq!(confirmed.confirmed_by.as_deref(), Some("owner"));
    assert_eq!(
        store.action_blocked(TENANT, PROJECT, "sdk").await.unwrap(),
        None
    );
}
