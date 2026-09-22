#![cfg(feature = "pg-tests")]
//! Regression coverage for execution responses with a one-connection pool.

use awr_team_pg::{ExecutionStore, LeaseStore, PgError};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

mod common;
use common::{fresh_team_schema, test_config, with_app_role};

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";
const ACTOR: &str = "actor-a";
const RUNNER: &str = "runner-a";
const CLIENT: &str = "client-a";

async fn prepare(store: &ExecutionStore, claim_id: &str, request_id: &str) -> String {
    tokio::time::timeout(
        Duration::from_secs(3),
        store.prepare(
            TENANT,
            PROJECT,
            ACTOR,
            CLIENT,
            request_id,
            claim_id,
            RUNNER,
            "contract-a",
            "input-a",
            "hard_fence",
            &["src/foo".into()],
            &json!([{"path": "src/foo/a.rs", "content": "fn a() {}"}]),
        ),
    )
    .await
    .expect("prepare must not wait for its own pool")
    .unwrap()
    .id
}

#[tokio::test]
async fn pool_size_one_returns_execution_responses_without_self_waiting() {
    // This integration-test binary owns its process environment. Keeping the
    // override here avoids cross-test mutation while exercising the supported
    // minimum pool capacity through the public stores.
    unsafe { std::env::set_var("AWR_TEAM_PG_POOL_MAX_SIZE", "1") };

    let (_guard, admin, db) = fresh_team_schema().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
                ('tenant-a','actor-a','agent','A','active'),
                ('tenant-a','runner-a','system','Runner','active');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');
             INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
                VALUES ('tenant-a','project-a','main','main','active');
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
                VALUES ('tenant-a','project-a','work-a','W');",
        )
        .await
        .unwrap();
    let config = with_app_role(&test_config(), &db);
    let store = Arc::new(ExecutionStore::from_config(config.clone()));
    let leases = LeaseStore::from_config(config);
    let session = leases
        .start_session(TENANT, PROJECT, ACTOR, CLIENT, "conv", "main", "work-a")
        .await
        .unwrap();
    let claim = leases
        .claim(TENANT, PROJECT, &session.id, ACTOR, CLIENT, "claim-1", 3600)
        .await
        .unwrap();

    let execution = prepare(&store, &claim.id, "prepare-main").await;

    // An early error rolls its transaction back and returns the sole pooled
    // connection; the next valid operation must still complete.
    let error = store
        .start(TENANT, PROJECT, &execution, claim.fence)
        .await
        .unwrap_err();
    assert!(matches!(error, PgError::Protocol(_)), "got {error}");

    let accepted = tokio::time::timeout(
        Duration::from_secs(3),
        store.accept(TENANT, PROJECT, &execution, claim.fence),
    )
    .await
    .expect("accept must not wait for its own committed connection")
    .unwrap();
    assert_eq!(accepted.state, "accepted");

    let accepted_replay = tokio::time::timeout(
        Duration::from_secs(3),
        store.accept(TENANT, PROJECT, &execution, claim.fence),
    )
    .await
    .expect("same-state accept must reuse the original transaction")
    .unwrap();
    assert_eq!(accepted_replay.state, "accepted");

    let running = tokio::time::timeout(
        Duration::from_secs(3),
        store.start(TENANT, PROJECT, &execution, claim.fence),
    )
    .await
    .expect("start must not wait for its own committed connection")
    .unwrap();
    assert_eq!(running.state, "running");

    let running_replay = tokio::time::timeout(
        Duration::from_secs(3),
        store.start(TENANT, PROJECT, &execution, claim.fence),
    )
    .await
    .expect("same-state start must reuse the original transaction")
    .unwrap();
    assert_eq!(running_replay.state, "running");

    let succeeded = tokio::time::timeout(
        Duration::from_secs(3),
        store.report(
            TENANT,
            PROJECT,
            RUNNER,
            "trusted_executor",
            &execution,
            "succeeded",
            json!({"output_digest": "output-a"}),
            &["src/foo/a.rs".into()],
        ),
    )
    .await
    .expect("report must not wait for its own committed connection")
    .unwrap();
    assert_eq!(succeeded.state, "succeeded");

    let report_replay = tokio::time::timeout(
        Duration::from_secs(3),
        store.report(
            TENANT,
            PROJECT,
            RUNNER,
            "trusted_executor",
            &execution,
            "succeeded",
            json!({"output_digest": "output-a"}),
            &["src/foo/a.rs".into()],
        ),
    )
    .await
    .expect("same-result report must reuse the original transaction")
    .unwrap();
    assert_eq!(report_replay.state, "succeeded");

    let late_cancel = tokio::time::timeout(
        Duration::from_secs(3),
        store.report(
            TENANT,
            PROJECT,
            RUNNER,
            "trusted_executor",
            &execution,
            "cancelled",
            json!({"reason": "late stop confirmation"}),
            &[],
        ),
    )
    .await
    .expect("late cancellation must reuse the original transaction")
    .unwrap();
    assert_eq!(late_cancel.state, "succeeded");
    assert!(late_cancel.cancel_requested);

    // A failed lookup is another transaction early-return path. The next read
    // proves that rollback returned the one connection and did not leak scope.
    let missing = store
        .get("tenant-other", PROJECT, &execution)
        .await
        .unwrap_err();
    assert!(
        matches!(missing, PgError::ProjectNotAvailable),
        "wrong-scope lookup returned {missing}"
    );
    assert_eq!(
        store.get(TENANT, PROJECT, &execution).await.unwrap().state,
        "succeeded"
    );

    // Two response-producing transitions contend for the same one-connection
    // pool. Neither request may retain the resource while acquiring again.
    let concurrent_a = prepare(&store, &claim.id, "prepare-concurrent-a").await;
    let concurrent_b = prepare(&store, &claim.id, "prepare-concurrent-b").await;
    let concurrent = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(
            store.accept(TENANT, PROJECT, &concurrent_a, claim.fence),
            store.accept(TENANT, PROJECT, &concurrent_b, claim.fence),
        )
    })
    .await
    .expect("concurrent responses must drain through a one-connection pool");
    assert_eq!(concurrent.0.unwrap().state, "accepted");
    assert_eq!(concurrent.1.unwrap().state, "accepted");

    let uncertain = prepare(&store, &claim.id, "prepare-unknown").await;
    store
        .accept(TENANT, PROJECT, &uncertain, claim.fence)
        .await
        .unwrap();
    store
        .start(TENANT, PROJECT, &uncertain, claim.fence)
        .await
        .unwrap();
    let unknown = tokio::time::timeout(
        Duration::from_secs(3),
        store.report(
            TENANT,
            PROJECT,
            RUNNER,
            "trusted_executor",
            &uncertain,
            "unknown",
            json!({"unknown_reason": "effect outcome unavailable"}),
            &[],
        ),
    )
    .await
    .expect("unknown report must not self-wait")
    .unwrap();
    assert_eq!(unknown.state, "unknown");

    let reconciled = tokio::time::timeout(
        Duration::from_secs(3),
        store.reconcile(
            TENANT,
            PROJECT,
            RUNNER,
            &uncertain,
            "failed",
            json!({"basis": "external observation"}),
            true,
        ),
    )
    .await
    .expect("reconcile must not wait for its own committed connection")
    .unwrap();
    assert_eq!(reconciled.state, "failed");

    let persisted = admin
        .query_one(
            "SELECT state, cancel_requested FROM awr_team.executions WHERE id=$1",
            &[&execution],
        )
        .await
        .unwrap();
    assert_eq!(persisted.get::<_, String>(0), "succeeded");
    assert!(persisted.get::<_, bool>(1));
    let recovered = admin
        .query_one(
            "SELECT e.state, w.recovery_blocked
             FROM awr_team.executions e
             JOIN awr_team.work_runtime w
               ON w.tenant_id=e.tenant_id AND w.project_id=e.project_id
              AND w.scope_id=e.scope_id AND w.work_id=e.work_id
             WHERE e.id=$1",
            &[&uncertain],
        )
        .await
        .unwrap();
    assert_eq!(recovered.get::<_, String>(0), "failed");
    assert!(!recovered.get::<_, bool>(1));
}
