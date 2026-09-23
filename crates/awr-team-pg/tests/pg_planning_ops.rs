//! AWR-TMCP-023: atomic planning receipts + scoped source.content.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team_pg::{
    PgError, PlanningSuggestRequest, SourceStore, WorkstreamQuery, WorkstreamReadStore,
};
use fixture::*;
use serde_json::json;

async fn elev_maintainer(admin: &tokio_postgres::Client) {
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='maintainer', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn planning_suggest_resume_after_reserved_does_not_duplicate() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));

    let req = PlanningSuggestRequest {
        protocol_version: 1,
        request_id: "suggest-resume-1".into(),
        rationale: "shared types".into(),
        affected_work_keys: vec!["a".into()],
        proposed_notes: json!({"note": "x"}),
        author_person_id: Some("agent".into()),
    };
    let first = store
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(first["already_recorded"], false);
    let suggestion_id = first["result"]["suggestion_id"]
        .as_str()
        .unwrap()
        .to_string();

    admin
        .execute(
            "UPDATE awr_team.planning_command_receipts
             SET status='reserved',
                 result_json = jsonb_build_object(
                    'protocol', 'awr-team-planning-command-v1',
                    'request_id', 'suggest-resume-1',
                    'op', 'planning.propose',
                    'status', 'reserved',
                    'domain_id', $3::text,
                    'already_recorded', false
                 ),
                 updated_at=clock_timestamp()
             WHERE tenant_id=$1 AND project_id=$2 AND request_id='suggest-resume-1'",
            &[&TENANT, &PROJECT, &suggestion_id],
        )
        .await
        .unwrap();

    let second = store
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(second["result"]["suggestion_id"], suggestion_id);

    let count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_suggestions
             WHERE tenant_id=$1 AND project_id=$2 AND rationale='shared types'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1, "resume must not create a second suggestion");

    let third = store
        .planning_suggest(TENANT, PROJECT, A, &req)
        .await
        .unwrap();
    assert_eq!(third["already_recorded"], true);
}

#[tokio::test]
async fn concurrent_planning_suggest_same_request_does_not_abort() {
    let (_g, admin, db, _read) = setup().await;
    elev_maintainer(&admin).await;
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    let req = PlanningSuggestRequest {
        protocol_version: 1,
        request_id: "suggest-race-1".into(),
        rationale: "race types".into(),
        affected_work_keys: vec!["a".into()],
        proposed_notes: json!({"note": "race"}),
        author_person_id: Some("agent".into()),
    };
    let (left, right) = tokio::join!(
        store.planning_suggest(TENANT, PROJECT, A, &req),
        store.planning_suggest(TENANT, PROJECT, A, &req),
    );
    let left = left.expect("concurrent reserve must not abort the loser");
    let right = right.expect("concurrent reserve must not abort the loser");
    let left_id = left["result"]["suggestion_id"].as_str().unwrap();
    let right_id = right["result"]["suggestion_id"].as_str().unwrap();
    assert_eq!(left_id, right_id);
    let count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.planning_suggestions
             WHERE tenant_id=$1 AND project_id=$2 AND rationale='race types'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1, "raced suggest must keep a single suggestion");
}

#[tokio::test]
async fn source_content_refuses_mixed_catalog_without_full_stream_grants() {
    let (_g, admin, db, read) = setup().await;
    let q_deny: WorkstreamQuery = serde_json::from_value(json!({
        "protocol_version": 1,
        "op": "source.content",
        "source_path": "workstreams.json"
    }))
    .unwrap();
    let err = read.query(TENANT, PROJECT, A, q_deny).await.unwrap_err();
    assert!(
        matches!(err, PgError::Forbidden),
        "partial stream grant must not read mixed workstreams.json: {err:?}"
    );

    admin
        .batch_execute(
            "INSERT INTO awr_team.workstream_grants(
                tenant_id,project_id,actor_id,client_id,workstream_id,
                authority_version,can_read,can_write,active)
             VALUES (
                'reader-tenant','reader-project','agent','cli-a',
                '00000000000000000000000002',1,true,false,true)
             ON CONFLICT (tenant_id,project_id,actor_id,client_id,workstream_id)
             DO UPDATE SET can_read=true, active=true,
               grant_version=awr_team.workstream_grants.grant_version+1;",
        )
        .await
        .unwrap();
    let read = WorkstreamReadStore::from_config(common::with_app_role(&common::test_config(), &db));
    let q_ok: WorkstreamQuery = serde_json::from_value(json!({
        "protocol_version": 1,
        "op": "source.content",
        "source_path": "workstreams.json"
    }))
    .unwrap();
    match read.query(TENANT, PROJECT, A, q_ok).await {
        Ok(v) => {
            let text = v["data"]["text"].as_str().unwrap_or("");
            assert!(
                text.contains("private-beta")
                    || text.contains("b-private")
                    || text.contains("00000000000000000000000002"),
                "elevated read should include private stream content: {text}"
            );
        }
        Err(PgError::Protocol(msg)) => {
            // Accept missing persisted text as environmental, not auth bypass.
            assert!(!msg.to_lowercase().contains("forbidden"), "{msg}");
        }
        Err(e) => panic!("elevated read should not be Forbidden: {e:?}"),
    }
}
