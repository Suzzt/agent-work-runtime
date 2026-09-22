#![cfg(feature = "pg-tests")]
mod common;
use awr_core::*;
use awr_team_pg::DeliveryAdoptionStore;
use common::{app_client, fresh_team_schema, test_config, with_app_role};
use std::sync::MutexGuard;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (MutexGuard<'static, ()>, DeliveryAdoptionStore, String) {
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
    (guard, DeliveryAdoptionStore::from_config(cfg), db)
}

fn digest(n: u8) -> String {
    format!("{n:064x}")
}
fn source_sha(n: u8) -> String {
    format!("{n:040x}")
}

fn requirement() -> DeliveryRequirement {
    DeliveryRequirement {
        provider: WorkstreamWorkBinding {
            project_id: PROJECT.into(),
            work_item_id: "upstream".into(),
            workstream_id: Id::from(1),
        },
        consumer: WorkstreamWorkBinding {
            project_id: PROJECT.into(),
            work_item_id: "downstream".into(),
            workstream_id: Id::from(2),
        },
        selected: DeliveryVersion {
            completion_receipt: Id::from(3),
            contract_sha256: digest(0xa),
            artifact_sha256: digest(0xb),
            source_sha: source_sha(0xc),
            environment: "candidate-v1".into(),
            acceptance_round: "round-1".into(),
            export_scope_sha256: digest(0xd),
        },
        policy: DeliveryVersionPolicy::FixedDelivery,
        minimum_level: EvidenceLevel::LocallyVerified,
    }
}

fn proof(req: &DeliveryRequirement) -> CompletionAcceptanceProof {
    CompletionAcceptanceProof {
        completion_receipt_id: req.selected.completion_receipt,
        work_item_id: req.provider.work_item_id.clone(),
        contract_sha256: req.selected.contract_sha256.clone(),
        artifact_sha256: req.selected.artifact_sha256.clone(),
        independence_kind: "team_independent".into(),
        team_independent_acceptance: true,
        author_person_id: "author".into(),
        reviewer_person_id: "reviewer".into(),
        evidence_id: Id::from(4),
        evidence_level: EvidenceLevel::LocallyVerified,
        verified_at_ms: 90,
    }
}

#[tokio::test]
async fn pg_register_grant_adopt_and_replay() {
    let (_guard, store, _) = setup().await;
    let req = requirement();
    let (dep, receipt) = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-1".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap();
    assert!(!receipt.replayed);
    let (_, replay) = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-1".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap();
    assert!(replay.replayed);

    let (export, _) = store
        .grant_export(
            TENANT,
            PROJECT,
            &GrantExportAuthorizationRequest {
                request_key: "ex-1".into(),
                authorization_id: "ea-1".into(),
                project_id: PROJECT.into(),
                provider_work_item_id: req.provider.work_item_id.clone(),
                delivery: req.selected.clone(),
                granted_by: "owner".into(),
                now_ms: 20,
            },
        )
        .await
        .unwrap();

    let mut bad = proof(&req);
    bad.team_independent_acceptance = false;
    bad.independence_kind = "author_self_report".into();
    assert!(
        store
            .adopt(
                TENANT,
                PROJECT,
                &AdoptDeliveryRequest {
                    request_key: "ad-bad".into(),
                    credential_id: "ac-bad".into(),
                    dependency: dep.clone(),
                    completion: bad,
                    export_authorization: export.clone(),
                    availability: DeliveryAvailability::Available,
                    current_selection: Some(req.selected.clone()),
                    now_ms: 100,
                },
            )
            .await
            .is_err()
    );

    let (cred, _) = store
        .adopt(
            TENANT,
            PROJECT,
            &AdoptDeliveryRequest {
                request_key: "ad-1".into(),
                credential_id: "ac-1".into(),
                dependency: dep,
                completion: proof(&req),
                export_authorization: export,
                availability: DeliveryAvailability::Available,
                current_selection: Some(req.selected.clone()),
                now_ms: 100,
            },
        )
        .await
        .unwrap();
    assert_eq!(cred.status, AdoptionCredentialStatus::Active);
    let loaded = store
        .get_credential(TENANT, PROJECT, "ac-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.completion_receipt_id, cred.completion_receipt_id);
}

#[tokio::test]
async fn pg_rls_blocks_unscoped_and_cross_tenant_app_reads() {
    let (_guard, store, db) = setup().await;
    let req = requirement();
    store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-1".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap();

    let app = app_client(&db).await;
    // No tenant context: FORCE RLS hides rows.
    let count: i64 = app
        .query_one(
            "SELECT count(*) FROM awr_team.hard_delivery_dependencies",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);

    // Wrong tenant still empty even if settings are forged for another tenant.
    app.batch_execute(
        "SELECT set_config('awr.tenant_id','tenant-b', false),
                set_config('awr.project_id','project-a', false)",
    )
    .await
    .unwrap();
    let count: i64 = app
        .query_one(
            "SELECT count(*) FROM awr_team.hard_delivery_dependencies",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);

    // Correct scope via store binding still returns the row.
    assert!(
        store
            .get_dependency(TENANT, PROJECT, "dep-1")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn pg_refuses_cross_project_consumer() {
    let (_guard, store, _) = setup().await;
    let mut req = requirement();
    req.consumer.project_id = "other".into();
    let err = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-x".into(),
                dependency_id: "dep-x".into(),
                provider: req.provider,
                consumer: req.consumer,
                selected: req.selected,
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("project") || msg.contains("binding"),
        "unexpected error: {msg}"
    );
}
