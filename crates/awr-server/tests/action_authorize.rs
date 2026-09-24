//! TMCP-011: HTTP/MCP service shares the PG action map and rejects forged identity.
use awr_server::service::{
    action_authorization_capabilities, command_action_name, query_action_name,
    reject_forged_authority_fields,
};
use awr_team::{Action, RoleTemplate, action_allowed_for_template};
use serde_json::json;

#[test]
fn service_action_map_matches_role_matrix_expectations() {
    assert_eq!(
        command_action_name("session.start"),
        Some("session.maintain_own")
    );
    assert_eq!(
        command_action_name("claim.release"),
        Some("claim.manage_own")
    );
    assert_eq!(
        command_action_name("execution.start"),
        Some("execution.request_and_report_own")
    );
    assert!(query_action_name("events.list") == Some("work.read"));

    assert!(action_allowed_for_template(
        RoleTemplate::Developer,
        Action::SessionMaintainOwn
    ));
    assert!(!action_allowed_for_template(
        RoleTemplate::Developer,
        Action::PlanningPublish
    ));
    assert!(!action_allowed_for_template(
        RoleTemplate::Reader,
        Action::ClaimManageOwn
    ));
    assert!(action_allowed_for_template(
        RoleTemplate::Maintainer,
        Action::PlanningPublish
    ));
    assert!(action_allowed_for_template(
        RoleTemplate::ProjectAdmin,
        Action::AccessManageProject
    ));

    let caps = action_authorization_capabilities();
    assert_eq!(caps["action_authorization"], "tmcp_010_shared_decision");
    assert_eq!(caps["tool_discovery_is_navigation_only"], true);
}

#[test]
fn http_body_cannot_smuggle_authority_claims() {
    assert!(
        reject_forged_authority_fields(&json!({
            "protocol_version": 1,
            "op": "session.start",
            "work_id": "a",
            "args": {"conversation_id": "c"}
        }))
        .is_ok()
    );
    for key in ["actor_id", "role", "grants", "tenant_id", "permissions"] {
        let mut body = json!({
            "protocol_version": 1,
            "op": "claim.acquire",
            "work_id": "a"
        });
        body[key] = json!("forged");
        assert!(
            reject_forged_authority_fields(&body).is_err(),
            "key {key} must be rejected"
        );
    }
}
