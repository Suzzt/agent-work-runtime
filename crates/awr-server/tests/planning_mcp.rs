#![cfg(feature = "pg-tests")]
#[path = "../../awr-team-pg/tests/common/mod.rs"]
mod common;
#[path = "../../awr-team-pg/tests/fixtures/workstream_access.rs"]
mod fixture;
use awr_server::service::{ProjectBinding, ServiceConfig};
use awr_team::{
    DraftChange, DraftDefinitionState, DraftOpKind, OrdinaryPlanningSelfApprovePolicy, TaskDraft,
};
use awr_team_pg::WorkstreamReadStore;
use fixture::*;
use rmcp::{
    RoleClient, ServiceExt,
    model::CallToolRequestParams,
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use std::time::Duration;

type Client = RunningService<RoleClient, ()>;

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn start(store: WorkstreamReadStore) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = ServiceConfig {
        version: 1,
        listen: address,
        allowed_hosts: vec![],
        allowed_web_origins: vec![],
        projects: vec![ProjectBinding {
            key: "one".into(),
            tenant_id: TENANT.into(),
            project_id: PROJECT.into(),
        }],
    };
    let router = awr_server::service::router(config, address, store).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        url: format!("http://{address}/v1/projects"),
        task,
    }
}
fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}
async fn connect(server: &Server, token: &str) -> Client {
    let transport = StreamableHttpClientTransport::with_client(
        http(),
        StreamableHttpClientTransportConfig::with_uri(format!("{}/one/mcp", server.url))
            .auth_header(token),
    );
    ().serve(transport).await.unwrap()
}
async fn call(client: &Client, name: &str, args: Value, error: bool) -> Value {
    let result = client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error.unwrap_or(false), error, "{result:?}");
    result.structured_content.unwrap()
}

fn draft(id: &str) -> TaskDraft {
    TaskDraft {
        work_id: id.into(),
        external_key: id.into(),
        title: format!("Task {id}"),
        goals: vec!["delivery".into()],
        scope_paths: vec!["specs/api.md".into()],
        acceptance: vec!["ok".into()],
        required_dependencies: vec![],
        completion_policy: "independent_review".into(),
        definition_state: DraftDefinitionState::Draft,
        split_from: None,
        split_children: vec![],
    }
}

#[tokio::test]
async fn planning_tools_catalog_and_suggest_idempotent_http_mcp_parity() {
    let (_g, admin, db, store) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='maintainer', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';",
        )
        .await
        .unwrap();
    let server = start(store).await;
    let client = connect(&server, A).await;

    let tools = client.list_tools(Default::default()).await.unwrap();
    let names: Vec<_> = tools.tools.iter().map(|t| t.name.to_string()).collect();
    for expected in [
        "awr_team_planning_suggest",
        "awr_team_planning_draft",
        "awr_team_planning_preview",
        "awr_team_planning_approve",
        "awr_team_planning_publish",
        "awr_team_planning_outcome",
    ] {
        assert!(names.iter().any(|n| n == expected), "{names:?}");
    }

    let caps = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"capabilities"}),
        false,
    )
    .await;
    assert_eq!(caps["artifact_content"], true);
    assert_eq!(caps["source_content"], true);
    assert_eq!(caps["planning_mcp"]["sql_tools"], false);
    assert_eq!(caps["planning_mcp"]["arbitrary_file_edit"], false);
    assert_eq!(caps["planning_mcp"]["direct_done"], false);

    // Forged identity refused.
    let denied = call(
        &client,
        "awr_team_planning_suggest",
        json!({
            "protocol_version":1,
            "request_id":"sug-1",
            "rationale":"x".repeat(8),
            "affected_work_keys":["a"],
            "actor_id":"forged"
        }),
        true,
    )
    .await;
    assert_eq!(denied["code"], "Forbidden");

    let sug = call(
        &client,
        "awr_team_planning_suggest",
        json!({
            "protocol_version":1,
            "request_id":"sug-stable-1",
            "rationale":"Need a shared dependency for the SDK",
            "affected_work_keys":["CLIENT-1"],
            "proposed_notes":{"add":"SHARED-1"}
        }),
        false,
    )
    .await;
    assert_eq!(sug["already_recorded"], false);
    assert_eq!(sug["op"], "planning.propose");
    assert_eq!(sug["result"]["claimable"], false);

    let replay = call(
        &client,
        "awr_team_planning_suggest",
        json!({
            "protocol_version":1,
            "request_id":"sug-stable-1",
            "rationale":"Need a shared dependency for the SDK",
            "affected_work_keys":["CLIENT-1"],
            "proposed_notes":{"add":"SHARED-1"}
        }),
        false,
    )
    .await;
    assert_eq!(replay["already_recorded"], true);

    let outcome = call(
        &client,
        "awr_team_planning_outcome",
        json!({"protocol_version":1,"request_id":"sug-stable-1"}),
        false,
    )
    .await;
    assert_eq!(outcome["already_recorded"], true);
    assert!(outcome["next_step"].as_str().unwrap().contains("reuse"));

    // HTTP parity for outcome
    let http_out = http()
        .post(format!("{}/one/planning/outcome", server.url))
        .header("authorization", format!("Bearer {A}"))
        .json(&json!({"protocol_version":1,"request_id":"sug-stable-1"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(http_out["request_id"], "sug-stable-1");
    assert_eq!(http_out["already_recorded"], true);

    // Query planning.outcome
    let q = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"planning.outcome","request_id":"sug-stable-1"}),
        false,
    )
    .await;
    assert_eq!(q["data"]["already_recorded"], true);

    // Path bypass refused on source.content
    let bad = call(
        &client,
        "awr_team_query",
        json!({"protocol_version":1,"op":"source.content","source_path":"../secret"}),
        true,
    )
    .await;
    assert!(
        bad["code"] == "InvalidInput" || bad["code"] == "Forbidden" || bad["code"] == "Unsupported",
        "{bad}"
    );

    let _ = (admin, db);
}

#[tokio::test]
async fn planning_draft_create_via_mcp_uses_business_entrypoint() {
    let (_g, admin, _db, store) = setup().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='maintainer', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
               ('reader-tenant','reader-project','API-1','API-1')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let server = start(store).await;
    let client = connect(&server, A).await;
    let after = draft("SHARED-1");
    let changes = vec![DraftChange {
        op: DraftOpKind::CreateTask,
        before: None,
        after,
    }];
    let body = json!({
        "protocol_version":1,
        "request_id":"draft-1",
        "mode":"create",
        "changes": changes,
        "allowed_spec_roots":["specs"],
        "project_goal_keys":["delivery"],
        "self_approve_policy": OrdinaryPlanningSelfApprovePolicy::ordinary_default(),
    });
    let created = call(&client, "awr_team_planning_draft", body, false).await;
    assert_eq!(created["op"], "planning.edit_draft");
    assert!(created["result"]["candidate_id"].as_str().unwrap().len() > 10);
    let preview = call(
        &client,
        "awr_team_planning_preview",
        json!({
            "protocol_version":1,
            "candidate_id": created["result"]["candidate_id"]
        }),
        false,
    )
    .await;
    assert!(
        preview.get("diff").is_some() || preview.get("candidate_id").is_some(),
        "{preview}"
    );
}
