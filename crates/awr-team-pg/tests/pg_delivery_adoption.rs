#![cfg(feature = "pg-tests")]
mod common;
use awr_core::*;
use awr_team_pg::DeliveryAdoptionStore;
use common::{app_client, fresh_team_schema, test_config, with_app_role};
use std::sync::MutexGuard;
use tokio_postgres::Client;

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";

async fn setup() -> (
    MutexGuard<'static, ()>,
    DeliveryAdoptionStore,
    String,
    Client,
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
    (guard, DeliveryAdoptionStore::from_config(cfg), db, admin)
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

/// Seed a currently-selected WS-018 completion with evidence + approved review.
async fn seed_ws018_completion(admin: &Client, req: &DeliveryRequirement) {
    let receipt_id = req.selected.completion_receipt.to_string();
    let evidence_id = Id::from(4).to_string();
    let contract = &req.selected.contract_sha256;
    let artifact = &req.selected.artifact_sha256;
    let bundle = digest(0xe);
    admin
        .batch_execute(
            "INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
                VALUES ('tenant-a','project-a','main','Main','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
                ('tenant-a','project-a','upstream','UP'),
                ('tenant-a','project-a','downstream','DOWN')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
                ('tenant-a','project-a','author','Author','active'),
                ('tenant-a','project-a','reviewer','Reviewer','active')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.evidence(
                tenant_id,project_id,id,work_id,contract_hash,output_digest,
                evidence_kind,trust_basis,digest,payload_json,created_by)
             VALUES ($1,$2,$3,'upstream',$4,$5,'artifact','trusted_executor',$6,'{}'::jsonb,'runner')",
            &[
                &TENANT,
                &PROJECT,
                &evidence_id,
                contract,
                artifact,
                &bundle,
            ],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.review_rounds(
                tenant_id,project_id,id,work_id,round_index,bundle_hash,contract_hash,
                author_actor_id,state,evidence_id)
             VALUES ($1,$2,'round-1','upstream',1,$3,$4,'author','approved',$5)",
            &[&TENANT, &PROJECT, &bundle, contract, &evidence_id],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.completion_receipts(
                tenant_id,project_id,id,work_id,scope_id,contract_hash,result_digest,
                dependency_binding_hash,evidence_bundle_hash,policy,approved_by_json,
                independence_kind,evidence_id,approved_by_person_id,submitted_by_person_id,
                accepted_at)
             VALUES ($1,$2,$3,'upstream','main',$4,$5,'deps',$5,'review','{}'::jsonb,
                     'team_independent',$6,'reviewer','author',
                     to_timestamp(0.09))",
            &[
                &TENANT,
                &PROJECT,
                &receipt_id,
                contract,
                &bundle,
                &evidence_id,
            ],
        )
        .await
        .unwrap();
    admin
        .execute(
            "INSERT INTO awr_team.work_runtime(
                tenant_id,project_id,scope_id,work_id,state,work_version,last_fence,
                selected_completion_id)
             VALUES ($1,$2,'main','upstream','completed',1,0,$3)",
            &[&TENANT, &PROJECT, &receipt_id],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn pg_adopt_refuses_without_ws018_completion() {
    let (_guard, store, _, _) = setup().await;
    let req = requirement();
    let (dep, _) = store
        .register_dependency(
            TENANT,
            PROJECT,
            &RegisterHardDependencyRequest {
                request_key: "reg-empty".into(),
                dependency_id: "dep-empty".into(),
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
    let (export, _) = store
        .grant_export(
            TENANT,
            PROJECT,
            &GrantExportAuthorizationRequest {
                request_key: "ex-empty".into(),
                authorization_id: "ea-empty".into(),
                project_id: PROJECT.into(),
                provider_work_item_id: req.provider.work_item_id.clone(),
                delivery: req.selected.clone(),
                granted_by: "owner".into(),
                now_ms: 20,
            },
        )
        .await
        .unwrap();

    let err = store
        .adopt(
            TENANT,
            PROJECT,
            &AdoptDeliveryRequest {
                request_key: "ad-empty".into(),
                credential_id: "ac-empty".into(),
                dependency: dep,
                completion: proof(&req),
                export_authorization: export,
                availability: DeliveryAvailability::Available,
                current_selection: Some(req.selected.clone()),
                now_ms: 100,
            },
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("trusted WS-018 completion proof missing"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn pg_register_grant_adopt_and_replay() {
    let (_guard, store, _, admin) = setup().await;
    let req = requirement();
    seed_ws018_completion(&admin, &req).await;

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

    // Request-supplied self-report fields must not matter: storage proof wins.
    let mut bad = proof(&req);
    bad.team_independent_acceptance = false;
    bad.independence_kind = "author_self_report".into();

    let (cred, _) = store
        .adopt(
            TENANT,
            PROJECT,
            &AdoptDeliveryRequest {
                request_key: "ad-1".into(),
                credential_id: "ac-1".into(),
                dependency: dep,
                completion: bad,
                export_authorization: export,
                availability: DeliveryAvailability::Available,
                current_selection: Some(req.selected.clone()),
                now_ms: 100,
            },
        )
        .await
        .unwrap();
    assert_eq!(cred.status, AdoptionCredentialStatus::Active);
    assert_eq!(cred.acceptance_author, "author");
    assert_eq!(cred.acceptance_reviewer, "reviewer");
    let loaded = store
        .get_credential(TENANT, PROJECT, "ac-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.completion_receipt_id, cred.completion_receipt_id);

    let count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.completion_receipts
             WHERE tenant_id=$1 AND project_id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);

    let (_, adopt_replay) = store
        .adopt(
            TENANT,
            PROJECT,
            &AdoptDeliveryRequest {
                request_key: "ad-1".into(),
                credential_id: "ac-1".into(),
                dependency: store
                    .get_dependency(TENANT, PROJECT, "dep-1")
                    .await
                    .unwrap()
                    .unwrap(),
                completion: proof(&req),
                export_authorization: store
                    .get_export(TENANT, PROJECT, "ea-1")
                    .await
                    .unwrap()
                    .unwrap(),
                availability: DeliveryAvailability::Available,
                current_selection: Some(req.selected.clone()),
                now_ms: 100,
            },
        )
        .await
        .unwrap();
    assert!(adopt_replay.replayed);
}

#[tokio::test]
async fn pg_rls_blocks_unscoped_and_cross_tenant_app_reads() {
    let (_guard, store, db, _) = setup().await;
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
    let (_guard, store, _, _) = setup().await;
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
