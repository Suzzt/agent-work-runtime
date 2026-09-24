//! HTTP/MCP domain-entry action authorization (AWR-TMCP-011).
//!
//! Identity and grants are resolved only inside PostgreSQL from the verified
//! bearer. This module exposes the shared command/query → TMCP-010 action map so
//! transports cannot invent a parallel decision, and rejects body fields that
//! attempt to forge actor/role/tenant authority.

use awr_team::{Action, PERMISSION_POLICY_ID, PERMISSION_POLICY_VERSION};
use awr_team_pg::{command_business_action, query_business_action};
use serde_json::{Value, json};

/// Capability advertisement merged into live `capabilities` responses via the
/// shared PG gate. Tool catalogs remain navigation-only.
pub fn action_authorization_capabilities() -> Value {
    json!({
        "action_authorization": "tmcp_010_shared_decision",
        "permission_policy_id": PERMISSION_POLICY_ID,
        "permission_policy_version": PERMISSION_POLICY_VERSION,
        "identity_source": "verified_bearer_credential",
        "body_cannot_forge_identity": true,
        "tool_discovery_is_navigation_only": true,
        "exact_replay_reuses_receipt": true,
        "changed_intent_or_expired_authority": "refused"
    })
}

/// Map a Team command op to its TMCP-010 action name when one applies.
pub fn command_action_name(op: &str) -> Option<&'static str> {
    command_business_action(op).map(Action::as_str)
}

/// Map a Team query op to `work.read` when supported.
pub fn query_action_name(op: &str) -> Option<&'static str> {
    query_business_action(op).map(Action::as_str)
}

const FORBIDDEN_AUTHORITY_KEYS: &[&str] = &[
    "actor_id",
    "actor",
    "client_id",
    "client",
    "tenant_id",
    "tenant",
    "project_id",
    "role",
    "role_template",
    "grants",
    "permissions",
    "allowed_actions",
    "authority_scope",
    "membership_role",
    "secret",
    "bearer",
    "credential",
];

/// Reject request objects that try to smuggle identity or permission claims.
/// Selectors such as work_id / workstream_id remain allowed; they never grant.
pub fn reject_forged_authority_fields(value: &Value) -> Result<(), &'static str> {
    let Some(obj) = value.as_object() else {
        return Ok(());
    };
    for key in FORBIDDEN_AUTHORITY_KEYS {
        if obj.contains_key(*key) {
            return Err("request cannot supply identity or permission fields");
        }
    }
    if let Some(args) = obj.get("args") {
        reject_forged_authority_fields(args)?;
    }
    Ok(())
}

/// Access-management tools may name a *subject* membership plan, but still
/// refuse caller-identity forgery and raw secret material in ordinary MCP args.
pub fn reject_access_management_forgeries(value: &Value) -> Result<(), &'static str> {
    const FORBIDDEN: &[&str] = &[
        "actor_id",
        "actor",
        "client_id",
        "client",
        "tenant_id",
        "tenant",
        "project_id",
        "secret",
        "bearer",
        "token",
        "raw_credential",
        "password",
        "authorization",
    ];
    fn walk(value: &Value) -> Result<(), &'static str> {
        let Some(obj) = value.as_object() else {
            return Ok(());
        };
        for key in FORBIDDEN {
            if obj.contains_key(*key) {
                return Err("request cannot supply caller identity or raw secrets");
            }
        }
        for (k, v) in obj {
            // Nested plan.subject is allowed; it is not caller authority.
            if k == "subject" {
                if let Some(subject) = v.as_object() {
                    for bad in ["secret", "bearer", "token", "raw_credential", "password"] {
                        if subject.contains_key(bad) {
                            return Err("request cannot supply caller identity or raw secrets");
                        }
                    }
                }
                continue;
            }
            if k == "plan" || k == "credential" || k == "grants" {
                walk(v)?;
                continue;
            }
            if v.is_object() || v.is_array() {
                match v {
                    Value::Object(_) => walk(v)?,
                    Value::Array(items) => {
                        for item in items {
                            walk(item)?;
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
    walk(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shared_maps_match_pg_exports() {
        assert_eq!(
            command_action_name("claim.acquire"),
            Some("claim.manage_own")
        );
        assert_eq!(
            command_action_name("execution.report"),
            Some("execution.request_and_report_own")
        );
        assert_eq!(command_action_name("execution.attest"), None);
        assert_eq!(query_action_name("work.search"), Some("work.read"));
        assert_eq!(query_action_name("nope"), None);
        let caps = action_authorization_capabilities();
        assert_eq!(caps["permission_policy_id"], PERMISSION_POLICY_ID);
        assert_eq!(caps["body_cannot_forge_identity"], true);
    }

    #[test]
    fn forged_identity_fields_are_rejected_in_body_and_args() {
        assert!(reject_forged_authority_fields(&json!({"op":"session.start"})).is_ok());
        assert!(
            reject_forged_authority_fields(&json!({"op":"session.start","actor_id":"x"})).is_err()
        );
        assert!(
            reject_forged_authority_fields(&json!({"op":"session.start","args":{"role":"admin"}}))
                .is_err()
        );
        assert!(
            reject_forged_authority_fields(&json!({"op":"work.list","work_id":"a","grants":[]}))
                .is_err()
        );
    }

    #[test]
    fn access_tools_allow_subject_plan_but_refuse_raw_secrets_and_caller_forgery() {
        let plan = json!({
            "protocol_version":1,
            "plan":{
                "protocol_version":1,
                "subject":{"id":"worker","kind":"agent","display_name":"W"},
                "subject_client_id":"cli",
                "role":"developer",
                "grants":[],
                "credential":{"id":"c1","secret_hash":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            }
        });
        assert!(reject_access_management_forgeries(&plan).is_ok());
        assert!(reject_access_management_forgeries(&json!({"tenant_id":"x","plan":{}})).is_err());
        assert!(reject_access_management_forgeries(&json!({"bearer":"awr1.x","plan":{}})).is_err());
        assert!(
            reject_access_management_forgeries(&json!({
                "plan":{"subject":{"id":"a","kind":"agent","display_name":"A","bearer":"nope"}}
            }))
            .is_err()
        );
    }
}
