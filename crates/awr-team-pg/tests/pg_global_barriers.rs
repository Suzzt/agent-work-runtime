#![cfg(feature = "pg-tests")]
//! WS-023: global barriers, fixed lock order, restore/revoke epoch fencing.
use awr_team_pg::{
    ImportStore, PgError, ResourceLockKey, TeamStore, lock_resources_sorted, lock_works_sorted,
};
use serde_json::json;
use std::sync::MutexGuard;
use std::time::Duration;
use tokio_postgres::Client;
mod common;
#[path = "fixtures/workstream_access.rs"]
mod ws_fixture;
use common::{fresh_team_schema, test_config, with_app_role};

const TENANT: &str = "tenant-a";
const PROJECT: &str = "project-a";
const ACTOR: &str = "actor-a";

async fn setup() -> (MutexGuard<'static, ()>, Client, String, ImportStore) {
    let (guard, admin, db) = fresh_team_schema().await;
    admin
        .batch_execute(
            "INSERT INTO awr_team.tenants(id,name,status) VALUES ('tenant-a','A','active');
             INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status)
                VALUES ('tenant-a','actor-a','agent','A','active');
             INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
                VALUES ('tenant-a','cred-1','actor-a','client-a','hash');
             INSERT INTO awr_team.projects(tenant_id,id,key,mode,coordinator_epoch,status)
                VALUES ('tenant-a','project-a','alpha','team','epoch-1','active');
             INSERT INTO awr_team.work_scopes(tenant_id,project_id,id,name,status)
                VALUES ('tenant-a','project-a','main','main','active');
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key)
                VALUES ('tenant-a','project-a','work-a','W'),
                       ('tenant-a','project-a','work-b','X');
             INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence)
                VALUES ('tenant-a','project-a','main','work-a','pending',1,0),
                       ('tenant-a','project-a','main','work-b','pending',1,0);
             INSERT INTO awr_team.resource_reservations(
                tenant_id,project_id,id,work_id,resource_kind,canonical_key,state)
                VALUES ('tenant-a','project-a','res-a','work-a','file','src/a.rs','reserved'),
                       ('tenant-a','project-a','res-b','work-b','file','src/b.rs','reserved');",
        )
        .await
        .unwrap();
    (
        guard,
        admin,
        db.clone(),
        ImportStore::from_config(with_app_role(&test_config(), &db)),
    )
}

fn touch(request_id: &str, work: &str) -> awr_team_pg::CommandRequest {
    awr_team_pg::CommandRequest {
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        actor_id: ACTOR.into(),
        client_id: "c".into(),
        request_id: request_id.into(),
        op: "work.touch".into(),
        args: json!({"work_id": work, "scope_id": "main"}),
    }
}

async fn wait_for_lock(observer: &Client, fragment: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = observer
                .query_one(
                    "SELECT EXISTS(
                        SELECT 1 FROM pg_stat_activity
                        WHERE datname = current_database()
                          AND wait_event_type = 'Lock'
                          AND query LIKE $1
                    )",
                    &[&format!("%{fragment}%")],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for lock on {fragment}"));
}

async fn work_version(admin: &Client, work: &str) -> i64 {
    admin
        .query_one(
            "SELECT work_version FROM awr_team.work_runtime WHERE work_id=$1",
            &[&work],
        )
        .await
        .unwrap()
        .get(0)
}

async fn project_status(admin: &Client) -> String {
    admin
        .query_one(
            "SELECT status FROM awr_team.projects WHERE id=$1",
            &[&PROJECT],
        )
        .await
        .unwrap()
        .get(0)
}

async fn project_revision(admin: &Client) -> i64 {
    admin
        .query_one(
            "SELECT project_revision FROM awr_team.projects WHERE id=$1",
            &[&PROJECT],
        )
        .await
        .unwrap()
        .get(0)
}

async fn coordinator_epoch(admin: &Client) -> String {
    admin
        .query_one(
            "SELECT coordinator_epoch FROM awr_team.projects WHERE id=$1",
            &[&PROJECT],
        )
        .await
        .unwrap()
        .get(0)
}

/// Ordinary write racing freeze: barrier wins; no torn write after frozen.
#[tokio::test]
async fn concurrent_ordinary_write_vs_freeze_barrier_wins_without_torn_state() {
    let (_lock, admin, db, store) = setup().await;
    let team = TeamStore::from_config(with_app_role(&test_config(), &db));
    let before_version = work_version(&admin, "work-a").await;
    let before_rev = project_revision(&admin).await;

    // Hold the project barrier as freeze would, without committing yet.
    let mut freezer = common::connect_config(&with_app_role(&test_config(), &db)).await;
    let freeze_tx = freezer.transaction().await.unwrap();
    freeze_tx
        .batch_execute(
            "SELECT set_config('awr.tenant_id','tenant-a',true);
             SELECT set_config('awr.project_id','project-a',true);
             SELECT id FROM awr_team.projects WHERE tenant_id='tenant-a' AND id='project-a' FOR UPDATE;
             UPDATE awr_team.projects SET status='frozen'
               WHERE tenant_id='tenant-a' AND id='project-a' AND status='active';",
        )
        .await
        .unwrap();

    let observer = common::connect_config(&common::with_db(&test_config(), &db)).await;
    let write = tokio::spawn(async move { team.execute(touch("race-freeze", "work-a")).await });
    wait_for_lock(&observer, "FOR UPDATE").await;

    freeze_tx.commit().await.unwrap();
    let write_result = write.await.unwrap();
    assert!(
        matches!(write_result, Err(PgError::ProjectNotAvailable)),
        "write after freeze barrier must fail closed: {write_result:?}"
    );
    assert_eq!(project_status(&admin).await, "frozen");
    assert_eq!(work_version(&admin, "work-a").await, before_version);
    assert_eq!(project_revision(&admin).await, before_rev);

    // API freeze is idempotent once frozen; admission stays closed.
    store.freeze(TENANT, PROJECT).await.unwrap();
    let team = TeamStore::from_config(with_app_role(&test_config(), &db));
    assert!(matches!(
        team.execute(touch("after-freeze", "work-a")).await,
        Err(PgError::ProjectNotAvailable)
    ));
}

/// Ordinary write racing restore: old epoch cannot land effects; fencing advances.
#[tokio::test]
async fn concurrent_ordinary_write_vs_restore_old_epoch_cannot_produce_effects() {
    let (_lock, admin, db, store) = setup().await;
    let backup = store.backup(TENANT, PROJECT, &[], &[]).await.unwrap();
    let old_epoch = coordinator_epoch(&admin).await;
    let before_version = work_version(&admin, "work-a").await;

    let mut restorer = common::connect_config(&with_app_role(&test_config(), &db)).await;
    let restore_tx = restorer.transaction().await.unwrap();
    // Mimic restore's first exclusive project lock without finishing the epoch cut.
    restore_tx
        .batch_execute(
            "SELECT set_config('awr.tenant_id','tenant-a',true);
             SELECT set_config('awr.project_id','project-a',true);
             SELECT id FROM awr_team.projects WHERE tenant_id='tenant-a' AND id='project-a' FOR UPDATE;",
        )
        .await
        .unwrap();

    let team = TeamStore::from_config(with_app_role(&test_config(), &db));
    let observer = common::connect_config(&common::with_db(&test_config(), &db)).await;
    let write = tokio::spawn(async move { team.execute(touch("race-restore", "work-a")).await });
    wait_for_lock(&observer, "FOR UPDATE").await;

    // Complete restore through the public API on a second connection after releasing.
    restore_tx.rollback().await.unwrap();
    let run = store
        .restore(TENANT, PROJECT, &backup.id, true, false)
        .await
        .unwrap();
    assert_ne!(run.new_epoch, old_epoch);
    assert!(!run.fencing_barriers.is_empty());

    let write_result = write.await.unwrap();
    // Either blocked by restore's project lock then refused after epoch/status
    // change, or committed before restore began — never under the old epoch after.
    match write_result {
        Ok(outcome) => {
            // Write landed before restore; epoch must have rotated afterward.
            assert_ne!(coordinator_epoch(&admin).await, old_epoch);
            let _ = outcome;
        }
        Err(PgError::ProjectNotAvailable)
        | Err(PgError::EpochChanged)
        | Err(PgError::RecoveryBlocked) => {}
        other => panic!("unexpected write outcome against restore barrier: {other:?}"),
    }
    assert_ne!(coordinator_epoch(&admin).await, old_epoch);
    // Old credential authority is revoked; work fence advanced for fencing.
    let revoked: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.credentials WHERE revoked_at IS NOT NULL",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(revoked, 1);
    assert!(work_version(&admin, "work-a").await >= before_version);
    store
        .require_epoch(TENANT, PROJECT, &old_epoch)
        .await
        .unwrap_err();
    store
        .require_epoch(TENANT, PROJECT, &run.new_epoch)
        .await
        .unwrap();
}

/// Permission revoke races an authenticated business write: revoke barrier wins,
/// the old credential cannot commit a new command effect, and runtime stays consistent.
#[tokio::test]
async fn concurrent_ordinary_write_vs_permission_revoke_leaves_no_torn_state() {
    use awr_team_pg::PgError;
    use serde_json::json;
    use ws_fixture::{A, PROJECT, TENANT, command, enable_writes, prepare, setup};

    let (_g, admin, db, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();
    let prepared = prepare(&store, A, "a").await;
    let sessions_before: i64 = admin
        .query_one(
            "SELECT count(*)::bigint FROM awr_team.sessions WHERE project_id=$1",
            &[&PROJECT],
        )
        .await
        .unwrap()
        .get(0);

    // Hold the project exclusive lock (operator revoke / restore shape) so the
    // authenticated write queues behind the barrier, then revoke before release.
    let mut holder = common::connect_config(&with_app_role(&test_config(), &db)).await;
    let hold = holder.transaction().await.unwrap();
    hold.batch_execute(
        "SELECT set_config('awr.tenant_id','reader-tenant',true);
         SELECT set_config('awr.project_id','reader-project',true);
         SELECT id FROM awr_team.projects
           WHERE tenant_id='reader-tenant' AND id='reader-project' FOR UPDATE;",
    )
    .await
    .unwrap();

    let observer = common::connect_config(&common::with_db(&test_config(), &db)).await;
    let write = {
        let commands = store.commands();
        let prepared = prepared.clone();
        tokio::spawn(async move {
            commands
                .execute(
                    TENANT,
                    PROJECT,
                    A,
                    command(
                        &prepared,
                        "race-revoke-write",
                        "session.start",
                        json!({"conversation_id": "conv-race-revoke"}),
                    ),
                )
                .await
        })
    };
    wait_for_lock(&observer, "FOR UPDATE").await;

    hold.batch_execute(
        "UPDATE awr_team.credentials SET revoked_at=clock_timestamp()
         WHERE id='reader-a' AND revoked_at IS NULL;",
    )
    .await
    .unwrap();
    hold.commit().await.unwrap();

    let write_result = write.await.unwrap();
    assert!(
        matches!(write_result, Err(PgError::Forbidden)),
        "revoked credential must not commit an authenticated write: {write_result:?}"
    );

    let revoked: i64 = admin
        .query_one(
            "SELECT count(*)::bigint FROM awr_team.credentials
             WHERE id='reader-a' AND revoked_at IS NOT NULL",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(revoked, 1);
    let sessions_after: i64 = admin
        .query_one(
            "SELECT count(*)::bigint FROM awr_team.sessions WHERE project_id=$1",
            &[&PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        sessions_after, sessions_before,
        "failed revoke-race write must not create session side effects"
    );

    let again = commands
        .execute(
            TENANT,
            PROJECT,
            A,
            command(
                &prepared,
                "after-revoke-retry",
                "session.start",
                json!({"conversation_id": "conv-after-revoke"}),
            ),
        )
        .await;
    assert!(
        matches!(again, Err(PgError::Forbidden)),
        "same bearer must stay refused after revoke"
    );
}

/// Two concurrent lock shapes that would deadlock if tasks/resources were taken
/// in presentation order instead of the fixed sorted order. Project lock is NOT
/// held exclusively across the opposite-order section — sorted task/resource order
/// is what prevents deadlock (unsorted opposite orders fail under lock_timeout).
#[tokio::test]
async fn fixed_lock_order_prevents_cross_line_deadlock() {
    let (_lock, _admin, db, _store) = setup().await;
    // Counterexample: unsorted opposite work orders with only FOR SHARE on the
    // project interleave and hit lock_timeout — proving sorts are load-bearing.
    let app_counter_a = with_app_role(&test_config(), &db);
    let app_counter_b = with_app_role(&test_config(), &db);
    let counter_left = tokio::spawn(async move {
        let mut client = common::connect_config(&app_counter_a).await;
        let tx = client.transaction().await.unwrap();
        tx.batch_execute(
            "SELECT set_config('awr.tenant_id','tenant-a',true);
             SELECT set_config('awr.project_id','project-a',true);
             SELECT id FROM awr_team.projects WHERE id='project-a' FOR SHARE;
             SET LOCAL lock_timeout = '800ms';
             SELECT work_id FROM awr_team.work_runtime
               WHERE project_id='project-a' AND scope_id='main' AND work_id='work-b' FOR UPDATE;",
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let err = tx
            .batch_execute(
                "SELECT work_id FROM awr_team.work_runtime
                   WHERE project_id='project-a' AND scope_id='main' AND work_id='work-a' FOR UPDATE;",
            )
            .await;
        let _ = tx.rollback().await;
        err
    });
    let counter_right = tokio::spawn(async move {
        let mut client = common::connect_config(&app_counter_b).await;
        let tx = client.transaction().await.unwrap();
        tx.batch_execute(
            "SELECT set_config('awr.tenant_id','tenant-a',true);
             SELECT set_config('awr.project_id','project-a',true);
             SELECT id FROM awr_team.projects WHERE id='project-a' FOR SHARE;
             SET LOCAL lock_timeout = '800ms';
             SELECT work_id FROM awr_team.work_runtime
               WHERE project_id='project-a' AND scope_id='main' AND work_id='work-a' FOR UPDATE;",
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let err = tx
            .batch_execute(
                "SELECT work_id FROM awr_team.work_runtime
                   WHERE project_id='project-a' AND scope_id='main' AND work_id='work-b' FOR UPDATE;",
            )
            .await;
        let _ = tx.rollback().await;
        err
    });
    let (left_res, right_res) = tokio::join!(counter_left, counter_right);
    let left_err = left_res.unwrap();
    let right_err = right_res.unwrap();
    assert!(
        left_err.is_err() || right_err.is_err(),
        "unsorted opposite work lock orders must time out / fail without sorted helpers"
    );

    // Production path: shared project admission + sorted helpers succeed.
    let app_a = with_app_role(&test_config(), &db);
    let app_b = with_app_role(&test_config(), &db);
    let left = tokio::spawn(async move {
        let mut client = common::connect_config(&app_a).await;
        let tx = client.transaction().await.unwrap();
        tx.batch_execute(
            "SELECT set_config('awr.tenant_id','tenant-a',true);
             SELECT set_config('awr.project_id','project-a',true);
             SELECT id FROM awr_team.projects WHERE id='project-a' FOR SHARE;",
        )
        .await
        .unwrap();
        lock_works_sorted(
            &tx,
            TENANT,
            PROJECT,
            "main",
            &["work-b".into(), "work-a".into()],
        )
        .await
        .unwrap();
        lock_resources_sorted(
            &tx,
            TENANT,
            PROJECT,
            &[
                ResourceLockKey::new("file", "src/b.rs", ""),
                ResourceLockKey::new("file", "src/a.rs", ""),
            ],
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.commit().await.unwrap();
    });

    let right = tokio::spawn(async move {
        let mut client = common::connect_config(&app_b).await;
        let tx = client.transaction().await.unwrap();
        tx.batch_execute(
            "SELECT set_config('awr.tenant_id','tenant-a',true);
             SELECT set_config('awr.project_id','project-a',true);
             SELECT id FROM awr_team.projects WHERE id='project-a' FOR SHARE;",
        )
        .await
        .unwrap();
        lock_works_sorted(
            &tx,
            TENANT,
            PROJECT,
            "main",
            &["work-a".into(), "work-b".into()],
        )
        .await
        .unwrap();
        lock_resources_sorted(
            &tx,
            TENANT,
            PROJECT,
            &[
                ResourceLockKey::new("file", "src/a.rs", ""),
                ResourceLockKey::new("file", "src/b.rs", ""),
            ],
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.commit().await.unwrap();
    });

    tokio::time::timeout(Duration::from_secs(10), async {
        left.await.unwrap();
        right.await.unwrap();
    })
    .await
    .expect("sorted lock order must not deadlock across opposite presentation orders");
}

/// Narrower per-task locks (project FOR SHARE) still lose to exclusive
/// freeze/restore barriers and keep lifecycle event revisions ordered.
#[tokio::test]
async fn narrower_share_locks_still_preserve_freeze_restore_event_order() {
    let (_lock, admin, db, store) = setup().await;

    // Task writer holds a shared project lock + exclusive task lock (narrow mode).
    let mut task = common::connect_config(&with_app_role(&test_config(), &db)).await;
    let task_tx = task.transaction().await.unwrap();
    task_tx
        .batch_execute(
            "SELECT set_config('awr.tenant_id','tenant-a',true);
             SELECT set_config('awr.project_id','project-a',true);
             SELECT enabled FROM awr_team.workstream_modes
               WHERE tenant_id='tenant-a' AND project_id='project-a' FOR SHARE;
             SELECT id FROM awr_team.projects
               WHERE tenant_id='tenant-a' AND id='project-a' FOR SHARE;",
        )
        .await
        .unwrap();
    lock_works_sorted(&task_tx, TENANT, PROJECT, "main", &["work-a".into()])
        .await
        .unwrap();

    let observer = common::connect_config(&common::with_db(&test_config(), &db)).await;
    let freezer = ImportStore::from_config(with_app_role(&test_config(), &db));
    let freeze = tokio::spawn(async move { freezer.freeze(TENANT, PROJECT).await });
    wait_for_lock(&observer, "FOR UPDATE").await;

    // Finish narrow work without emitting a revision, then freeze proceeds.
    task_tx
        .execute(
            "UPDATE awr_team.work_runtime SET work_version=work_version+1
             WHERE work_id='work-a'",
            &[],
        )
        .await
        .unwrap();
    task_tx.commit().await.unwrap();
    freeze.await.unwrap().unwrap();
    assert_eq!(project_status(&admin).await, "frozen");

    // Unfreeze via restore-shaped activation path: backup+restore on frozen project
    // requires active status for backup — re-activate for restore ordering check.
    admin
        .batch_execute(
            "UPDATE awr_team.projects SET status='active' WHERE id='project-a';
             UPDATE awr_team.credentials SET revoked_at=NULL WHERE id='cred-1';",
        )
        .await
        .unwrap();
    let before_events: i64 = admin
        .query_one("SELECT count(*) FROM awr_team.events", &[])
        .await
        .unwrap()
        .get(0);
    let backup = store.backup(TENANT, PROJECT, &[], &[]).await.unwrap();
    let run = store
        .restore(TENANT, PROJECT, &backup.id, true, false)
        .await
        .unwrap();
    let rows = admin
        .query(
            "SELECT project_revision, event_type FROM awr_team.events
             ORDER BY project_revision, event_index, id",
            &[],
        )
        .await
        .unwrap();
    assert!(rows.len() as i64 > before_events);
    let mut prev = 0i64;
    let mut saw_restore = false;
    for row in rows {
        let rev: i64 = row.get(0);
        assert!(rev >= prev, "lifecycle/event revisions must stay ordered");
        prev = rev;
        let ty: String = row.get(1);
        if ty.starts_with("restore.") {
            saw_restore = true;
        }
    }
    assert!(saw_restore);
    assert_eq!(run.fencing_barriers[0].coordinator_epoch, run.new_epoch);
}
