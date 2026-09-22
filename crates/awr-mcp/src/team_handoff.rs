//! Local MCP surface for confirmed Team handoff package validation (WS-017).
//! Authenticated propose/inspect/accept/reject/cancel/timeout run on Team HTTP/MCP
//! (`awr_team_command` / `awr_team_query` via awr-server). This tool only validates
//! a handoff package and explains duty projections so hosts re-prepare context.
use awr_core::{HandoffPackage, HandoffStatus, TeamHandoff};
use rmcp::model::{CallToolResult, Tool, ToolAnnotations};
use serde_json::{Value, json};

pub(crate) fn tool() -> Tool {
    let schema = json!({
        "type":"object","additionalProperties":false,
        "required":["action"],
        "properties":{
            "action":{"type":"string","enum":["capabilities","validate_package","explain_duty"]},
            "package":{"type":"object","description":"HandoffPackage JSON for validate_package"},
            "handoff":{"type":"object","description":"TeamHandoff JSON for explain_duty"},
            "now_ms":{"type":"integer"}
        }
    });
    let mut tool = Tool::new(
        "awr_team_handoff",
        "Validate a confirmed Team handoff package or explain duty for a handoff snapshot. Does not mutate Team state; use authenticated awr_team_command handoff.* ops for propose/inspect/accept/reject/cancel/timeout. Receiver must re-prepare context; timeout does not stop execution.",
        schema.as_object().unwrap().clone(),
    );
    tool.annotations = Some(
        ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    );
    tool
}

pub(crate) fn handle(args: Value) -> Result<CallToolResult, awr_core::Error> {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| awr_core::Error::InvalidInput("action required".into()))?;
    match action {
        "capabilities" => Ok(CallToolResult::structured(json!({
            "protocol":"awr-team-handoff-local-v1",
            "team_commands":[
                "handoff.propose","handoff.inspect","handoff.accept",
                "handoff.reject","handoff.cancel","handoff.timeout"
            ],
            "team_queries":["handoff.inspect"],
            "kinds":["execution","responsibility"],
            "statuses":["proposed","inspected","accepted","rejected","cancelled","timed_out"],
            "rules":{
                "execution_vs_responsibility":"separate ops",
                "accept_requires":["context_reprepared","prior_stopped_or_reconciled","no_unknown_executions","fresh_fence"],
                "timeout_does_not_stop_execution":true,
                "receiver_must_reprepare_context":true,
                "exactly_one_concurrent_accept":true
            }
        }))),
        "validate_package" => {
            let package: HandoffPackage = serde_json::from_value(
                args.get("package")
                    .cloned()
                    .ok_or_else(|| awr_core::Error::InvalidInput("package required".into()))?,
            )
            .map_err(|e| awr_core::Error::InvalidInput(e.to_string()))?;
            package.validate()?;
            Ok(CallToolResult::structured(json!({
                "ok":true,
                "context_requires_refresh":true,
                "note":"package structurally valid; receiver must still re-prepare live context before accept"
            })))
        }
        "explain_duty" => {
            let handoff: TeamHandoff = serde_json::from_value(
                args.get("handoff")
                    .cloned()
                    .ok_or_else(|| awr_core::Error::InvalidInput("handoff required".into()))?,
            )
            .map_err(|e| awr_core::Error::InvalidInput(e.to_string()))?;
            let now = args
                .get("now_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(handoff.updated_at_ms);
            let duty = handoff.duty_at(now)?;
            Ok(CallToolResult::structured(json!({
                "status": match handoff.status {
                    HandoffStatus::Proposed => "proposed",
                    HandoffStatus::Inspected => "inspected",
                    HandoffStatus::Accepted => "accepted",
                    HandoffStatus::Rejected => "rejected",
                    HandoffStatus::Cancelled => "cancelled",
                    HandoffStatus::TimedOut => "timed_out",
                },
                "duty": duty,
                "timeout_does_not_stop_execution": true
            })))
        }
        _ => Err(awr_core::Error::InvalidInput(
            "unsupported awr_team_handoff action".into(),
        )),
    }
}
