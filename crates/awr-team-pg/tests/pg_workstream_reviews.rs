#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team::WorkContract;
use awr_team_pg::{PgError, WorkstreamReadStore, workstream_credential_hash};
use fixture::*;
use serde_json::{Value, json};
use tokio_postgres::Client;

const RUNNER: &str =
    "awr1.runner-sys.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const REVIEWER_TOKEN: &str =
    "awr1.reviewer-h.eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const INPUT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RESULT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn seed_review_actors(admin: &Client) {
    admin
        .batch_execute(
            "INSERT INTO awr_team.actors(tenant_id,id,kind,display_name,status) VALUES
                ('reader-tenant','runner','system','Runner','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.project_memberships(tenant_id,project_id,actor_id,role) VALUES
                ('reader-tenant','reader-project','runner','worker')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.persons(tenant_id,project_id,id,display_name,status) VALUES
                ('reader-tenant','reader-project','person-author','Author','active'),
                ('reader-tenant','reader-project','reviewer','Reviewer','active')
             ON CONFLICT DO NOTHING;
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status) VALUES
                ('reader-tenant','reader-project','bind-agent','person-author','agent','active')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    for (id, actor, client, token) in [
        ("runner-sys", "runner", "cli-runner", RUNNER),
        ("reviewer-h", "reviewer", "cli-reviewer", REVIEWER_TOKEN),
    ] {
        let hash = workstream_credential_hash(token).unwrap();
        admin
            .execute(
                "INSERT INTO awr_team.credentials(tenant_id,id,actor_id,client_id,secret_hash)
                 VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
                &[&TENANT, &id, &actor, &client, &hash],
            )
            .await
            .unwrap();
    }
    let stream: String = admin
        .query_one(
            "SELECT workstream_id FROM awr_team.sessions WHERE id='session-a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    for (client, actor) in [("cli-runner", "runner"), ("cli-reviewer", "reviewer")] {
        admin
            .execute(
                "INSERT INTO awr_team.workstream_grants(
                    tenant_id,project_id,actor_id,client_id,workstream_id,
                    authority_version,can_read,can_write)
                 VALUES($1,$2,$3,$4,$5,1,true,true)
                 ON CONFLICT DO NOTHING",
                &[&TENANT, &PROJECT, &actor, &client, &stream],
            )
            .await
            .unwrap();
    }
    for (session, actor, client) in [
        ("session-runner", "runner", "cli-runner"),
        ("session-reviewer", "reviewer", "cli-reviewer"),
    ] {
        admin
            .execute(
                "INSERT INTO awr_team.sessions(
                    tenant_id,project_id,id,scope_id,work_id,actor_id,client_id,
                    conversation_id,state,workstream_id,ownership_version)
                 VALUES($1,$2,$3,'main','a',$4,$5,$3,'active',$6,1)
                 ON CONFLICT DO NOTHING",
                &[&TENANT, &PROJECT, &session, &actor, &client, &stream],
            )
            .await
            .unwrap();
    }
    enable_writes(admin).await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships
             SET role='developer', independent_review=true,
                 membership_version=membership_version+1
             WHERE tenant_id='reader-tenant' AND project_id='reader-project'
               AND actor_id='reviewer';
             UPDATE awr_team.workstream_grants
             SET can_write=true, grant_version=grant_version+1
             WHERE client_id IN ('cli-runner','cli-reviewer')",
        )
        .await
        .unwrap();
}

async fn insert_succeeded_execution(admin: &Client, id: &str, contract_hash: &str, fence: i64) {
    admin
        .execute(
            "INSERT INTO awr_team.executions(
                tenant_id,project_id,id,work_id,fence,contract_hash,input_digest,
                executor_actor_id,state,result_digest,scope_id)
             VALUES($1,$2,$3,'a',$4,$5,$6,'runner','succeeded',$7,'main')",
            &[
                &TENANT,
                &PROJECT,
                &id,
                &fence,
                &contract_hash,
                &INPUT,
                &RESULT,
            ],
        )
        .await
        .unwrap();
}

async fn run(
    store: &WorkstreamReadStore,
    token: &str,
    request: &str,
    op: &str,
    args: Value,
) -> Value {
    let prepared = prepare(store, token, "a").await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(&prepared, request, op, args),
        )
        .await
        .unwrap()["receipt"]["data"]
        .clone()
}

async fn run_err(
    store: &WorkstreamReadStore,
    token: &str,
    request: &str,
    op: &str,
    args: Value,
) -> PgError {
    let prepared = prepare(store, token, "a").await;
    store
        .commands()
        .execute(
            TENANT,
            PROJECT,
            token,
            command(&prepared, request, op, args),
        )
        .await
        .unwrap_err()
}

fn submit_args(session: &str, execution_id: &str, artifact: &str) -> Value {
    json!({
        "session_id": session,
        "expected_session_version": "1",
        "payload": {"passed": true, "output_digest": RESULT},
        "artifact_hex": artifact,
        "input_digest": INPUT,
        "dirty_tree": false,
        "execution_id": execution_id
    })
}

async fn current_contract(admin: &Client) -> WorkContract {
    let row = admin
        .query_one(
            "SELECT contract_json FROM awr_team.work_contracts
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id='a'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap();
    serde_json::from_value(row.get(0)).unwrap()
}

async fn put_contract(admin: &Client, contract: &WorkContract) {
    let hash = contract.hash().unwrap();
    let json = serde_json::to_value(contract).unwrap();
    admin
        .execute(
            "UPDATE awr_team.work_contracts
             SET contract_json=$3, contract_hash=$4
             WHERE tenant_id=$1 AND project_id=$2 AND scope_id='main' AND work_id='a'",
            &[&TENANT, &PROJECT, &json, &hash],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn mainline_submit_open_accept_complete_is_team_independent() {
    let (_g, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-main", &contract_hash, 1).await;

    let evidence = run(
        &store,
        RUNNER,
        "ev-1",
        "evidence.submit",
        submit_args(
            "session-runner",
            "exec-main",
            &hex_encode(b"ws018-artifact"),
        ),
    )
    .await;
    assert_eq!(evidence["trust_basis"], "trusted_executor");
    assert_eq!(evidence["author_self_report"], false);
    assert_eq!(evidence["task_complete"], false);
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();

    let opened = run(
        &store,
        A,
        "open-1",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id
        }),
    )
    .await;
    assert_eq!(opened["state"], "open");
    assert_eq!(opened["author_person_id"], "person-author");
    assert_eq!(
        opened["binds_exact_contract_artifact_execution_round"],
        true
    );
    let round_id = opened["round_id"].as_str().unwrap().to_string();

    let accepted = run(
        &store,
        REVIEWER_TOKEN,
        "acc-1",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round_id,
            "reason":"looks good"
        }),
    )
    .await;
    assert_eq!(accepted["state"], "approved");
    assert_eq!(accepted["independence_kind"], "team_independent");
    assert_eq!(accepted["team_independent_acceptance"], true);
    assert_eq!(accepted["human_approval"], true);
    assert_eq!(accepted["task_complete"], false);

    let completed = run(
        &store,
        A,
        "done-1",
        "work.complete",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id,
            "context_complete": true
        }),
    )
    .await;
    assert_eq!(completed["task_complete"], true);
    assert_eq!(completed["execution_success"], true);
    assert_eq!(completed["human_approval"], true);
    assert_eq!(completed["independence_kind"], "team_independent");
    assert_eq!(completed["team_independent_acceptance"], true);
    assert!(completed["provider_private_session"].is_null());
    assert_eq!(completed["selected_completion_id"], completed["receipt_id"]);

    let mut q = query("completion.inspect");
    q.work_id = Some("a".into());
    let inspected = store.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone();
    assert_eq!(inspected["runtime_state"], "completed");
    assert_eq!(
        inspected["completion"]["independence_kind"],
        "team_independent"
    );
    assert!(inspected["completion"]["provider_private_session"].is_null());
}

#[tokio::test]
async fn same_person_agent_cannot_fake_team_independence() {
    let (_g, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    admin
        .batch_execute(
            "DELETE FROM awr_team.person_agent_bindings
             WHERE tenant_id='reader-tenant' AND project_id='reader-project' AND agent_id='agent';
             INSERT INTO awr_team.person_agent_bindings(tenant_id,project_id,id,person_id,agent_id,status)
             VALUES ('reader-tenant','reader-project','bind-same','reviewer','agent','active');",
        )
        .await
        .unwrap();
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-same", &contract_hash, 1).await;
    let evidence = run(
        &store,
        RUNNER,
        "ev-same",
        "evidence.submit",
        submit_args("session-runner", "exec-same", &hex_encode(b"same-person")),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();
    let opened = run(
        &store,
        A,
        "open-same",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id
        }),
    )
    .await;
    assert_eq!(opened["author_person_id"], "reviewer");
    let round_id = opened["round_id"].as_str().unwrap().to_string();
    let err = run_err(
        &store,
        REVIEWER_TOKEN,
        "acc-same",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round_id,
            "reason":"self approve blocked"
        }),
    )
    .await;
    assert!(matches!(err, PgError::AuthorCannotReview));

    let mut contract = current_contract(&admin).await;
    contract.completion_policy = "trusted_execution_and_author_self_review".into();
    put_contract(&admin, &contract).await;
    let hash = contract.hash().unwrap();
    insert_succeeded_execution(&admin, "exec-self", &hash, 2).await;
    let evidence = run(
        &store,
        RUNNER,
        "ev-self",
        "evidence.submit",
        submit_args(
            "session-runner",
            "exec-self",
            &hex_encode(b"self-review-bytes"),
        ),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();
    let opened = run(
        &store,
        A,
        "open-self",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id
        }),
    )
    .await;
    let round_id = opened["round_id"].as_str().unwrap().to_string();
    let accepted = run(
        &store,
        REVIEWER_TOKEN,
        "acc-self",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round_id,
            "reason":"explicit self review policy"
        }),
    )
    .await;
    assert_eq!(accepted["independence_kind"], "personal_self_review");
    assert_eq!(accepted["team_independent_acceptance"], false);
}

#[tokio::test]
async fn return_rework_keeps_history_and_contract_change_blocks_stale_approval() {
    let (_g, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-ret", &contract_hash, 1).await;
    let evidence = run(
        &store,
        RUNNER,
        "ev-ret",
        "evidence.submit",
        submit_args("session-runner", "exec-ret", &hex_encode(b"return-bytes")),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();
    let opened = run(
        &store,
        A,
        "open-ret",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id
        }),
    )
    .await;
    let round_id = opened["round_id"].as_str().unwrap().to_string();
    let returned = run(
        &store,
        REVIEWER_TOKEN,
        "ret-1",
        "review.return",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round_id,
            "reason":"needs fixes"
        }),
    )
    .await;
    assert_eq!(returned["state"], "rejected");
    let rework = run(
        &store,
        A,
        "rew-1",
        "work.rework",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "round_id": round_id,
            "note":"addressing return"
        }),
    )
    .await;
    assert_eq!(rework["history_retained"], true);
    let count: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3 AND state='rejected'",
            &[&TENANT, &PROJECT, &round_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);

    let evidence2 = run(
        &store,
        RUNNER,
        "ev-ret2",
        "evidence.submit",
        submit_args("session-runner", "exec-ret", &hex_encode(b"return-bytes-2")),
    )
    .await;
    let evidence_id2 = evidence2["evidence_id"].as_str().unwrap().to_string();
    let opened2 = run(
        &store,
        A,
        "open-ret2",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id2
        }),
    )
    .await;
    let round2 = opened2["round_id"].as_str().unwrap().to_string();
    run(
        &store,
        REVIEWER_TOKEN,
        "acc-ret2",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round2,
            "reason":"ok for now"
        }),
    )
    .await;
    let mut contract = current_contract(&admin).await;
    contract.acceptance.push("extra-gate".into());
    put_contract(&admin, &contract).await;
    let err = run_err(
        &store,
        A,
        "done-stale",
        "work.complete",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id2,
            "context_complete": true
        }),
    )
    .await;
    assert!(matches!(
        err,
        PgError::EvidenceInvalid | PgError::ReviewRequired | PgError::CompletionRejected
    ));
}

#[tokio::test]
async fn completion_keeps_author_executor_distinct_from_finalizer_without_pr() {
    let (_g, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-attr", &contract_hash, 1).await;

    let evidence = run(
        &store,
        RUNNER,
        "ev-attr",
        "evidence.submit",
        submit_args(
            "session-runner",
            "exec-attr",
            &hex_encode(b"ws031-attr-artifact"),
        ),
    )
    .await;
    let evidence_id = evidence["evidence_id"].as_str().unwrap().to_string();
    let opened = run(
        &store,
        A,
        "open-attr",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id
        }),
    )
    .await;
    let round_id = opened["round_id"].as_str().unwrap().to_string();
    run(
        &store,
        REVIEWER_TOKEN,
        "acc-attr",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round_id,
            "reason":"independent approve"
        }),
    )
    .await;

    // Finalizer is agent (A), distinct from runner author/executor and reviewer.
    let completed = run(
        &store,
        A,
        "done-attr",
        "work.complete",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id,
            "context_complete": true
        }),
    )
    .await;
    assert_eq!(completed["author_actor_id"], "runner");
    assert_eq!(completed["executor_actor_id"], "runner");
    assert_eq!(completed["reviewer_actor_id"], "reviewer");
    assert_eq!(completed["final_submitter_actor_id"], "agent");
    assert_ne!(
        completed["author_actor_id"],
        completed["final_submitter_actor_id"]
    );
    assert_ne!(completed["executor_actor_id"], completed["execution_id"]);

    let mut q = query("completion.inspect");
    q.work_id = Some("a".into());
    let inspected = store.query(TENANT, PROJECT, A, q).await.unwrap()["data"].clone();
    let completion = &inspected["completion"];
    assert_eq!(completion["author_actor_id"], "runner");
    assert_eq!(completion["executor_actor_id"], "runner");
    assert_eq!(completion["final_submitter_actor_id"], "agent");
    assert_eq!(completion["approved_by"]["author_actor_id"], "runner");
    assert_eq!(completion["approved_by"]["executor_actor_id"], "runner");
    assert_eq!(completion["approved_by"]["reviewer_actor_id"], "reviewer");
    assert_eq!(
        completion["approved_by"]["final_submitter_actor_id"],
        "agent"
    );
    assert!(completion["approved_by"]["pr_delivery_id"].is_null());

    let row = admin
        .query_one(
            "SELECT author_actor_id, executor_actor_id, final_submitter_actor_id, execution_id
             FROM awr_team.completion_receipts
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[
                &TENANT,
                &PROJECT,
                &completed["receipt_id"].as_str().unwrap(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, Option<String>>(0).as_deref(), Some("runner"));
    assert_eq!(row.get::<_, Option<String>>(1).as_deref(), Some("runner"));
    assert_eq!(row.get::<_, Option<String>>(2).as_deref(), Some("agent"));
    assert_eq!(
        row.get::<_, Option<String>>(3).as_deref(),
        Some("exec-attr")
    );
    assert_ne!(
        row.get::<_, Option<String>>(1).as_deref(),
        row.get::<_, Option<String>>(3).as_deref()
    );
}

#[tokio::test]
async fn invalidated_prior_approval_cannot_complete() {
    let (_g, admin, _, store) = setup().await;
    seed_review_actors(&admin).await;
    let prepared = prepare(&store, A, "a").await;
    let contract_hash = prepared["data"]["contract_hash"]
        .as_str()
        .unwrap()
        .to_string();
    insert_succeeded_execution(&admin, "exec-inv", &contract_hash, 1).await;

    let evidence1 = run(
        &store,
        RUNNER,
        "ev-inv-1",
        "evidence.submit",
        submit_args("session-runner", "exec-inv", &hex_encode(b"invalidated-e1")),
    )
    .await;
    let evidence_id1 = evidence1["evidence_id"].as_str().unwrap().to_string();
    let opened1 = run(
        &store,
        A,
        "open-inv-1",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id1
        }),
    )
    .await;
    let round1 = opened1["round_id"].as_str().unwrap().to_string();
    run(
        &store,
        REVIEWER_TOKEN,
        "acc-inv-1",
        "review.accept",
        json!({
            "session_id":"session-reviewer",
            "expected_session_version":"1",
            "round_id": round1,
            "reason":"approve e1"
        }),
    )
    .await;

    let evidence2 = run(
        &store,
        RUNNER,
        "ev-inv-2",
        "evidence.submit",
        submit_args("session-runner", "exec-inv", &hex_encode(b"invalidated-e2")),
    )
    .await;
    let evidence_id2 = evidence2["evidence_id"].as_str().unwrap().to_string();
    run(
        &store,
        A,
        "open-inv-2",
        "review.open",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id2
        }),
    )
    .await;

    let r1_state: String = admin
        .query_one(
            "SELECT state FROM awr_team.review_rounds
             WHERE tenant_id=$1 AND project_id=$2 AND id=$3",
            &[&TENANT, &PROJECT, &round1],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(r1_state, "invalidated");

    let err = run_err(
        &store,
        A,
        "done-inv-stale",
        "work.complete",
        json!({
            "session_id":"session-a",
            "expected_session_version":"1",
            "evidence_id": evidence_id1,
            "context_complete": true
        }),
    )
    .await;
    assert!(matches!(err, PgError::ReviewRequired));

    let completed: i64 = admin
        .query_one(
            "SELECT count(*) FROM awr_team.work_runtime
             WHERE tenant_id=$1 AND project_id=$2 AND work_id='a' AND state='completed'",
            &[&TENANT, &PROJECT],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(completed, 0);
}
