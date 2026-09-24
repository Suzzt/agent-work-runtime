//! Named agent host adapter catalog surface for the Team HTTP/MCP service (WS-024).
//!
//! Exposes capability negotiation without treating AWR admission/cancel/session-end
//! as process start/kill. Concrete adapters live in `awr_runtime::host_adapter`.

use awr_core::{
    AdapterCapability, AdapterCapabilityMatrix, AdapterId, AdapterNegotiationRequest,
    AdapterNegotiationResult, negotiate_adapter,
};
use awr_runtime::host_adapter::{AdapterRegistry, built_in_registry};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::OnceLock;

fn registry() -> &'static AdapterRegistry {
    static REG: OnceLock<AdapterRegistry> = OnceLock::new();
    REG.get_or_init(built_in_registry)
}

/// Capability advertisement included in service discovery / operator surfaces.
pub fn named_agent_host_capabilities() -> Value {
    let matrices = registry().matrices();
    let named: Vec<&AdapterCapabilityMatrix> = matrices
        .iter()
        .filter(|m| {
            matches!(
                m.control_mode,
                awr_core::AdapterControlMode::NamedControlled
            )
        })
        .collect();
    json!({
        "version": 1,
        "work": "AWR-WS-024",
        "coordination_not_process_control": [
            "awr_admission_is_not_process_start",
            "awr_cancel_request_is_not_process_kill",
            "awr_session_end_is_not_process_kill"
        ],
        "l0_manual_report": "manual_report",
        "named_controlled_clients": named.iter().map(|m| m.adapter_id.as_str()).collect::<Vec<_>>(),
        "adapters": matrices.iter().map(|m| json!({
            "adapter_id": m.adapter_id.as_str(),
            "display_name": m.display_name,
            "control_mode": m.control_mode,
            "auto_startable": m.auto_startable,
            "capabilities": m.capabilities.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
            "human_continuation": m.human_continuation,
        })).collect::<Vec<_>>(),
        "subtask_parallelism": {
            "independent_tasks_parallel": true,
            "dependencies_order_start": true,
            "own_claims_and_resource_bounds": true,
            "parent_rollup_copies_artifacts": false,
            "parent_exit_autocompletes_children": false,
            "parent_exit_releases_unknown_children": false,
            "concurrency_cap_supported": true,
            "user_pause_supported": true,
            "reconnect_before_retry": true
        }
    })
}

pub fn negotiate_named_adapter(
    adapter_id: &str,
    required: &[AdapterCapability],
    optional: &[AdapterCapability],
) -> AdapterNegotiationResult {
    let request = AdapterNegotiationRequest {
        adapter_id: AdapterId::new(adapter_id).unwrap_or_else(|_| AdapterId(adapter_id.into())),
        required: required.iter().copied().collect::<BTreeSet<_>>(),
        optional: optional.iter().copied().collect::<BTreeSet<_>>(),
    };
    let matrix = registry().get(adapter_id).map(|a| a.matrix().clone());
    negotiate_adapter(matrix.as_ref(), &request)
}

pub fn usable_named_clients() -> Vec<String> {
    registry()
        .named_controlled()
        .into_iter()
        .map(|a| a.adapter_id().as_str().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::NegotiationDecision;

    #[test]
    fn advertises_two_named_clients_and_l0() {
        let caps = named_agent_host_capabilities();
        let named = caps["named_controlled_clients"].as_array().unwrap();
        assert_eq!(named.len(), 2);
        assert_eq!(caps["l0_manual_report"], "manual_report");
        assert_eq!(
            caps["subtask_parallelism"]["parent_rollup_copies_artifacts"],
            false
        );
    }

    #[test]
    fn negotiate_codex_and_claude() {
        let codex = negotiate_named_adapter(
            "codex_cli",
            &[AdapterCapability::StatusRead],
            &[AdapterCapability::Start],
        );
        assert_eq!(codex.decision, NegotiationDecision::Usable);
        let claude = negotiate_named_adapter("claude_code", &[AdapterCapability::StatusRead], &[]);
        assert_eq!(claude.decision, NegotiationDecision::Usable);
        assert!(usable_named_clients().contains(&"codex_cli".into()));
        assert!(usable_named_clients().contains(&"claude_code".into()));
    }
}
