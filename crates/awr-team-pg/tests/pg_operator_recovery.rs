#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;
use awr_team_pg::{
    ExecutionAttributionEntry, ExecutionAttributionPlan, OperatorBackup,
    OperatorExecutionAttribution, OperatorHistory, OperatorQuarantine, OperatorRecovery, PgError,
    WorkstreamCommand, WorkstreamReadStore,
};
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

async fn acquire(store: &WorkstreamReadStore, request: &str) -> WorkstreamCommand {
    let p = prepare(store, A, "a").await;
    command(
        &p,
        request,
        "claim.acquire",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "expected_work_version":p["data"]["runtime"]["work_version"].as_str().unwrap_or("0"),
            "ttl_seconds":60
        }),
    )
}

async fn seed_unattributed_history(admin: &Client) {
    // Safe subset for history-migration: inactive claim + session + work-bound event.
    // Bind to work_id `a` which ownership already maps to workstream 1.
    admin
        .batch_execute(
            "INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state)
            VALUES('reader-tenant','reader-project','session-legacy','main','a','agent','cli-a','session-legacy','ended');
            INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,fence,expires_at,state)
            VALUES('reader-tenant','reader-project','claim-legacy','main','a','session-legacy','agent',1,clock_timestamp()-interval '1 day','expired');
            INSERT INTO awr_team.events(tenant_id,project_id,id,project_revision,event_index,event_type,actor_id,work_id,payload_json)
            VALUES('reader-tenant','reader-project','ev-legacy',900,0,'session.ended','agent','a','{}');
            -- Active unattributed claim + nonterminal unattributed execution for quarantine / attribution.
            INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state)
            VALUES('reader-tenant','reader-project','session-active-u','main','a','agent','cli-a','session-active-u','active');
            INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,fence,expires_at,state)
            VALUES('reader-tenant','reader-project','claim-active-u','main','a','session-active-u','agent',2,clock_timestamp()+interval '1 day','active');
            INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,session_id,claim_id,fence,contract_hash,executor_actor_id,state,scope_id)
            VALUES('reader-tenant','reader-project','exec-u','a','session-active-u','claim-active-u',2,'contract','agent','running','main');
            INSERT INTO awr_team.work_runtime(tenant_id,project_id,scope_id,work_id,state,work_version,last_fence,recovery_blocked)
            VALUES('reader-tenant','reader-project','main','a','running',1,2,TRUE)
            ON CONFLICT (tenant_id,project_id,scope_id,work_id) DO UPDATE
              SET recovery_blocked=TRUE,last_fence=EXCLUDED.last_fence,work_version=work_runtime.work_version+1;",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn recovery_inspect_is_read_only_and_app_role_is_forbidden() {
    let (_g, mut admin, db, _) = setup().await;
    seed_unattributed_history(&admin).await;
    let before: Value = admin
        .query_one(
            "SELECT jsonb_build_object(
                'sessions',(SELECT count(*) FROM awr_team.sessions),
                'claims',(SELECT count(*) FROM awr_team.claims),
                'executions',(SELECT count(*) FROM awr_team.executions),
                'events',(SELECT count(*) FROM awr_team.events),
                'revision',(SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2))",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    let report = OperatorRecovery::inspect(&mut admin, TENANT, PROJECT)
        .await
        .unwrap();
    assert_eq!(report["protocol"], "awr-operator-recovery-inspect-v1");
    assert_eq!(report["read_only"], true);
    assert_eq!(report["mutation"], false);
    assert_eq!(report["restore"], false);
    assert_eq!(report["execution_authorized"], false);
    assert_eq!(report["findings"]["recovery_blocked_work"]["count"], 1);
    assert!(
        report["findings"]["unattributed_history"]["sessions_without_workstream"]
            .as_i64()
            .unwrap()
            >= 2
    );
    assert!(
        report["findings"]["unattributed_history"]["claims_without_workstream"]
            .as_i64()
            .unwrap()
            >= 2
    );
    assert_eq!(
        report["findings"]["unattributed_history"]["executions_without_workstream"],
        1
    );
    assert_eq!(report["findings"]["active_claims"], 1);
    assert!(
        report["findings"]["nonterminal_executions"]["count"]
            .as_i64()
            .unwrap()
            >= 1
    );
    let after: Value = admin
        .query_one(
            "SELECT jsonb_build_object(
                'sessions',(SELECT count(*) FROM awr_team.sessions),
                'claims',(SELECT count(*) FROM awr_team.claims),
                'executions',(SELECT count(*) FROM awr_team.executions),
                'events',(SELECT count(*) FROM awr_team.events),
                'revision',(SELECT project_revision FROM awr_team.projects WHERE tenant_id=$1 AND id=$2))",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(after, before);

    let mut app = common::app_client(&db).await;
    assert!(matches!(
        OperatorRecovery::inspect(&mut app, TENANT, PROJECT).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn history_migration_attributes_safe_subset_and_refuses_active_or_executions() {
    let (_g, mut admin, db, _) = setup().await;
    seed_unattributed_history(&admin).await;
    let preview = OperatorHistory::preview(&mut admin, TENANT, PROJECT)
        .await
        .unwrap();
    assert_eq!(preview["protocol"], "awr-operator-history-migration-v1");
    assert_eq!(preview["applied"], false);
    let attributable = preview["attributable"].as_array().unwrap();
    assert!(
        attributable
            .iter()
            .any(|i| i["kind"] == "session" && i["id"] == "session-legacy")
    );
    assert!(
        attributable
            .iter()
            .any(|i| i["kind"] == "claim" && i["id"] == "claim-legacy")
    );
    assert!(
        attributable
            .iter()
            .any(|i| i["kind"] == "event" && i["id"] == "ev-legacy")
    );
    let refused = preview["refused"].as_array().unwrap();
    assert!(refused.iter().any(|i| {
        i["kind"] == "claim"
            && i["id"] == "claim-active-u"
            && i["reason"] == "active_unattributed_claim_requires_manual_recovery"
    }));
    assert!(refused.iter().any(|i| {
        i["kind"] == "execution"
            && i["reason"] == "use_execution_attribution_protocol_for_reviewed_executor_client_id"
    }));

    let state = preview["state_digest"].as_str().unwrap();
    let plan = preview["plan_digest"].as_str().unwrap();
    let applied = OperatorHistory::apply(&mut admin, TENANT, PROJECT, "hist-1", state, plan)
        .await
        .unwrap();
    assert_eq!(applied["replayed"], false);
    assert_eq!(applied["receipt"]["completion_receipts_modified"], false);
    assert_eq!(applied["receipt"]["executions_modified"], false);
    assert_eq!(applied["receipt"]["identity_forged"], false);
    assert_eq!(applied["receipt"]["execution_authorized"], false);

    let ws: Option<String> = admin
        .query_one(
            "SELECT workstream_id FROM awr_team.sessions WHERE id='session-legacy'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(ws.is_some());
    let claim_ws: Option<String> = admin
        .query_one(
            "SELECT workstream_id FROM awr_team.claims WHERE id='claim-legacy'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(claim_ws.is_some());
    // Active claim and execution remain unattributed.
    assert_eq!(
        admin
            .query_one(
                "SELECT workstream_id FROM awr_team.claims WHERE id='claim-active-u'",
                &[],
            )
            .await
            .unwrap()
            .get::<_, Option<String>>(0),
        None
    );
    assert_eq!(
        admin
            .query_one(
                "SELECT workstream_id FROM awr_team.executions WHERE id='exec-u'",
                &[],
            )
            .await
            .unwrap()
            .get::<_, Option<String>>(0),
        None
    );

    let replay = OperatorHistory::apply(&mut admin, TENANT, PROJECT, "hist-1", state, plan)
        .await
        .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        OperatorHistory::outcome(&mut admin, TENANT, PROJECT, "hist-1")
            .await
            .unwrap()["outcome"],
        "committed"
    );
    assert_eq!(
        OperatorHistory::outcome(&mut admin, TENANT, PROJECT, "missing")
            .await
            .unwrap()["outcome"],
        "unknown"
    );

    let mut app = common::app_client(&db).await;
    assert!(matches!(
        OperatorHistory::preview(&mut app, TENANT, PROJECT).await,
        Err(PgError::Forbidden)
    ));
    for sql in [
        "SELECT * FROM awr_team.history_migrations",
        "DELETE FROM awr_team.history_migrations",
    ] {
        assert_eq!(
            app.batch_execute(sql).await.unwrap_err().code(),
            Some(&tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE)
        );
    }
}

#[tokio::test]
async fn quarantine_attributes_or_releases_active_claims_and_unknowns_unattributed_executions() {
    let (_g, mut admin, db, _) = setup().await;
    seed_unattributed_history(&admin).await;
    // Attribute the safe history first so quarantine focuses on active claim + execution.
    let hist = OperatorHistory::preview(&mut admin, TENANT, PROJECT)
        .await
        .unwrap();
    OperatorHistory::apply(
        &mut admin,
        TENANT,
        PROJECT,
        "hist-q",
        hist["state_digest"].as_str().unwrap(),
        hist["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();

    let preview = OperatorQuarantine::preview(&mut admin, TENANT, PROJECT, "release")
        .await
        .unwrap();
    assert_eq!(
        preview["protocol"],
        "awr-operator-claim-execution-recovery-v1"
    );
    let actionable = preview["actionable"].as_array().unwrap();
    assert!(actionable.iter().any(|i| {
        i["kind"] == "claim"
            && i["id"] == "claim-active-u"
            && i["action"] == "attribute_and_release"
    }));
    assert!(actionable.iter().any(|i| {
        i["kind"] == "execution" && i["id"] == "exec-u" && i["action"] == "quarantine_unknown"
    }));

    let applied = OperatorQuarantine::apply(
        &mut admin,
        TENANT,
        PROJECT,
        "q-1",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
        "release",
    )
    .await
    .unwrap();
    assert_eq!(applied["replayed"], false);
    assert_eq!(applied["receipt"]["identity_forged"], false);
    assert_eq!(applied["receipt"]["execution_authorized"], false);

    let claim = admin
        .query_one(
            "SELECT state,workstream_id FROM awr_team.claims WHERE id='claim-active-u'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(claim.get::<_, String>(0), "released");
    assert!(claim.get::<_, Option<String>>(1).is_some());
    let exec = admin
        .query_one(
            "SELECT state,workstream_id,executor_client_id,cancel_requested,unknown_reason FROM awr_team.executions WHERE id='exec-u'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(exec.get::<_, String>(0), "unknown");
    assert_eq!(exec.get::<_, Option<String>>(1), None);
    assert_eq!(exec.get::<_, Option<String>>(2), None);
    assert_eq!(exec.get::<_, bool>(3), true);
    assert_eq!(
        exec.get::<_, Option<String>>(4).as_deref(),
        Some("operator_quarantine")
    );
    let blocked: bool = admin
        .query_one(
            "SELECT recovery_blocked FROM awr_team.work_runtime WHERE work_id='a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(blocked);

    assert_eq!(
        OperatorQuarantine::apply(
            &mut admin,
            TENANT,
            PROJECT,
            "q-1",
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
            "release",
        )
        .await
        .unwrap()["replayed"],
        true
    );
    assert_eq!(
        OperatorQuarantine::outcome(&mut admin, TENANT, PROJECT, "q-1")
            .await
            .unwrap()["outcome"],
        "committed"
    );

    let mut app = common::app_client(&db).await;
    assert!(matches!(
        OperatorQuarantine::preview(&mut app, TENANT, PROJECT, "release").await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn execution_attribution_binds_reviewed_client_id_without_forging_identity() {
    let (_g, mut admin, db, _) = setup().await;
    // Dedicated CHECK-safe unattributed execution with matching session client_id.
    admin
        .batch_execute(
            "INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state)
            VALUES('reader-tenant','reader-project','session-attr','main','a','agent','cli-a','session-attr','ended');
            INSERT INTO awr_team.claims(tenant_id,project_id,id,scope_id,work_id,session_id,actor_id,fence,expires_at,state)
            VALUES('reader-tenant','reader-project','claim-attr','main','a','session-attr','agent',3,clock_timestamp()-interval '1 hour','released');
            INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,session_id,claim_id,fence,contract_hash,executor_actor_id,state,scope_id)
            VALUES('reader-tenant','reader-project','exec-attr','a','session-attr','claim-attr',3,'contract','agent','unknown','main');",
        )
        .await
        .unwrap();

    let plan = ExecutionAttributionPlan {
        protocol_version: 1,
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        attributions: vec![ExecutionAttributionEntry {
            execution_id: "exec-attr".into(),
            executor_client_id: "cli-a".into(),
        }],
    };
    let preview = OperatorExecutionAttribution::preview(&mut admin, &plan)
        .await
        .unwrap();
    assert_eq!(preview["protocol"], "awr-operator-execution-attribution-v1");
    let actionable = preview["actionable"].as_array().unwrap();
    assert_eq!(actionable.len(), 1);
    assert_eq!(actionable[0]["action"], "attribute");
    assert_eq!(actionable[0]["executor_client_id"], "cli-a");
    assert_eq!(actionable[0]["executor_client_id_invented"], false);

    let applied = OperatorExecutionAttribution::apply(
        &mut admin,
        &plan,
        "attr-1",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(applied["replayed"], false);
    assert_eq!(applied["receipt"]["identity_forged"], false);
    assert_eq!(applied["receipt"]["executor_client_id_invented"], false);
    assert_eq!(applied["receipt"]["completion_receipts_modified"], false);

    let row = admin
        .query_one(
            "SELECT workstream_id,executor_client_id,ownership_version IS NOT NULL FROM awr_team.executions WHERE id='exec-attr'",
            &[],
        )
        .await
        .unwrap();
    assert!(row.get::<_, Option<String>>(0).is_some());
    assert_eq!(row.get::<_, Option<String>>(1).as_deref(), Some("cli-a"));
    assert!(row.get::<_, bool>(2));

    // Mismatching reviewed client is refused without mutation.
    admin
        .batch_execute(
            "INSERT INTO awr_team.executions(tenant_id,project_id,id,work_id,session_id,claim_id,fence,contract_hash,executor_actor_id,state,scope_id)
            VALUES('reader-tenant','reader-project','exec-bad','a','session-attr','claim-attr',4,'contract','agent','failed','main');",
        )
        .await
        .unwrap();
    let bad = ExecutionAttributionPlan {
        protocol_version: 1,
        tenant_id: TENANT.into(),
        project_id: PROJECT.into(),
        attributions: vec![ExecutionAttributionEntry {
            execution_id: "exec-bad".into(),
            executor_client_id: "cli-b".into(),
        }],
    };
    let refused = OperatorExecutionAttribution::preview(&mut admin, &bad)
        .await
        .unwrap();
    assert!(
        refused["refused"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["reason"] == "reviewed_executor_client_id_mismatch_session")
    );
    assert!(refused["actionable"].as_array().unwrap().is_empty());

    assert_eq!(
        OperatorExecutionAttribution::outcome(&mut admin, TENANT, PROJECT, "attr-1")
            .await
            .unwrap()["outcome"],
        "committed"
    );

    let mut app = common::app_client(&db).await;
    assert!(matches!(
        OperatorExecutionAttribution::preview(&mut app, &plan).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn backup_fencing_restore_and_bounded_rebuild_against_real_pg() {
    let (_g, mut admin, db, _) = setup().await;
    // Attributed fixture has no unattributed history → backup allowed.
    let backed = OperatorBackup::backup(&mut admin, TENANT, PROJECT)
        .await
        .unwrap();
    assert_eq!(backed["workstreams_enabled"], true);
    let backup_id = backed["backup_id"].as_str().unwrap().to_string();
    let inspected = OperatorBackup::inspect(&mut admin, TENANT, PROJECT, &backup_id)
        .await
        .unwrap();
    assert_eq!(inspected["manifest_hash"], backed["manifest_hash"]);
    assert_eq!(inspected["read_only"], true);

    let preview = OperatorBackup::restore_preview(&mut admin, TENANT, PROJECT, &backup_id)
        .await
        .unwrap();
    assert_eq!(preview["decision"]["safe_to_apply"], true);
    assert_eq!(preview["decision"]["mode"], "verified_fencing");
    let old_epoch: String = admin
        .query_one(
            "SELECT coordinator_epoch FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);

    let restored = OperatorBackup::restore_apply(
        &mut admin,
        TENANT,
        PROJECT,
        &backup_id,
        "restore-1",
        preview["state_digest"].as_str().unwrap(),
        preview["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(restored["replayed"], false);
    assert_eq!(restored["receipt"]["completion_receipts_modified"], false);
    assert_eq!(restored["receipt"]["identity_forged"], false);
    assert_eq!(restored["receipt"]["execution_authorized"], false);
    assert_eq!(restored["receipt"]["report"]["mode"], "verified_fencing");
    let new_epoch: String = admin
        .query_one(
            "SELECT coordinator_epoch FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_ne!(new_epoch, old_epoch);
    assert_eq!(
        admin
            .query_one(
                "SELECT count(*) FROM awr_team.sessions WHERE state='active' AND project_id=$1",
                &[&PROJECT],
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert_eq!(
        OperatorBackup::restore_outcome(&mut admin, TENANT, PROJECT, "restore-1")
            .await
            .unwrap()["outcome"],
        "committed"
    );

    // Bounded rebuild: clear ownership after fencing quiet (work_items remain for coverage).
    admin
        .batch_execute(
            "DELETE FROM awr_team.workstream_ownership WHERE tenant_id='reader-tenant' AND project_id='reader-project'",
        )
        .await
        .unwrap();
    let rebuild = OperatorBackup::rebuild_preview(&mut admin, TENANT, PROJECT, &backup_id)
        .await
        .unwrap();
    assert_eq!(rebuild["decision"]["safe_to_apply"], true);
    assert_eq!(rebuild["decision"]["mode"], "ownership_materialize");
    assert!(rebuild["decision"]["insert_ownership"].as_u64().unwrap() >= 1);

    let rebuilt = OperatorBackup::rebuild_apply(
        &mut admin,
        TENANT,
        PROJECT,
        &backup_id,
        "rebuild-1",
        rebuild["state_digest"].as_str().unwrap(),
        rebuild["plan_digest"].as_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(rebuilt["replayed"], false);
    assert_eq!(rebuilt["receipt"]["completion_receipts_modified"], false);
    assert_eq!(rebuilt["receipt"]["identity_forged"], false);
    let own_count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.workstream_ownership WHERE project_id=$1",
            &[&PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert!(own_count >= 1);
    assert_eq!(
        OperatorBackup::rebuild_outcome(&mut admin, TENANT, PROJECT, "rebuild-1")
            .await
            .unwrap()["outcome"],
        "committed"
    );

    // Unattributed history blocks a fresh backup.
    admin
        .batch_execute(
            "INSERT INTO awr_team.sessions(tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,conversation_id,state)
            VALUES('reader-tenant','reader-project','session-block','main','a','agent','cli-a','session-block','ended');",
        )
        .await
        .unwrap();
    assert!(matches!(
        OperatorBackup::backup(&mut admin, TENANT, PROJECT).await,
        Err(PgError::Unsupported(_))
    ));

    let mut app = common::app_client(&db).await;
    assert!(matches!(
        OperatorBackup::backup(&mut app, TENANT, PROJECT).await,
        Err(PgError::Forbidden)
    ));
    assert!(matches!(
        OperatorBackup::inspect(&mut app, TENANT, PROJECT, &backup_id).await,
        Err(PgError::Forbidden)
    ));
}

#[tokio::test]
async fn quarantine_keeps_claim_acquire_recovery_blocked_for_false_and_true_barriers() {
    let (_g, mut admin, _db, store) = setup().await;
    enable_writes(&admin).await;
    let commands = store.commands();

    for (case, initial_blocked) in [("barrier_false", false), ("barrier_true", true)] {
        let exec_id = format!("exec-q-{case}");
        // Isolate work `a` to one legacy running unattributed execution.
        admin
            .batch_execute(
                "UPDATE awr_team.claims SET state='released' WHERE work_id='a' AND state='active';
                 DELETE FROM awr_team.executions WHERE work_id='a';
                 DELETE FROM awr_team.resource_reservations WHERE work_id='a';",
            )
            .await
            .unwrap();
        admin
            .execute(
                "INSERT INTO awr_team.executions(
                    tenant_id,project_id,id,work_id,session_id,claim_id,fence,contract_hash,
                    executor_actor_id,state,scope_id)
                 VALUES($1,$2,$3,'a','session-a',NULL,9,'contract','agent','running','main')",
                &[&TENANT, &PROJECT, &exec_id],
            )
            .await
            .unwrap();
        admin
            .execute(
                "INSERT INTO awr_team.work_runtime(
                    tenant_id,project_id,scope_id,work_id,state,work_version,last_fence,recovery_blocked)
                 VALUES($1,$2,'main','a','running',1,9,$3)
                 ON CONFLICT (tenant_id,project_id,scope_id,work_id) DO UPDATE
                   SET recovery_blocked=EXCLUDED.recovery_blocked,
                       last_fence=EXCLUDED.last_fence,
                       work_version=GREATEST(awr_team.work_runtime.work_version, EXCLUDED.work_version),
                       state='running',
                       selected_completion_id=NULL",
                &[&TENANT, &PROJECT, &initial_blocked],
            )
            .await
            .unwrap();

        assert!(
            matches!(
                commands
                    .execute(
                        TENANT,
                        PROJECT,
                        A,
                        acquire(&store, &format!("pre-{case}")).await
                    )
                    .await,
                Err(PgError::RecoveryBlocked)
            ),
            "{case}: pre-quarantine acquire must be RecoveryBlocked"
        );

        let preview = OperatorQuarantine::preview(&mut admin, TENANT, PROJECT, "release")
            .await
            .unwrap();
        assert!(
            preview["actionable"].as_array().unwrap().iter().any(|i| {
                i["kind"] == "execution"
                    && i["id"] == exec_id
                    && i["action"] == "quarantine_unknown"
                    && i["target_state"] == "unknown"
            }),
            "{case}: preview must plan quarantine_unknown"
        );
        OperatorQuarantine::apply(
            &mut admin,
            TENANT,
            PROJECT,
            &format!("q-{case}"),
            preview["state_digest"].as_str().unwrap(),
            preview["plan_digest"].as_str().unwrap(),
            "release",
        )
        .await
        .unwrap();

        let state: String = admin
            .query_one(
                "SELECT state FROM awr_team.executions WHERE id=$1",
                &[&exec_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(state, "unknown", "{case}");
        let blocked: bool = admin
            .query_one(
                "SELECT recovery_blocked FROM awr_team.work_runtime WHERE work_id='a'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert!(blocked, "{case}: recovery barrier must be set");

        assert!(
            matches!(
                commands
                    .execute(
                        TENANT,
                        PROJECT,
                        A,
                        acquire(&store, &format!("post-{case}")).await
                    )
                    .await,
                Err(PgError::RecoveryBlocked)
            ),
            "{case}: post-quarantine acquire must stay RecoveryBlocked"
        );
    }
}
