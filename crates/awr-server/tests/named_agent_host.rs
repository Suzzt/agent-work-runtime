use awr_core::{AdapterCapability, NegotiationDecision};
use awr_server::named_agent_host::{
    named_agent_host_capabilities, negotiate_named_adapter, usable_named_clients,
};

#[test]
fn server_advertises_two_usable_named_clients() {
    let clients = usable_named_clients();
    assert!(clients.contains(&"codex_cli".to_string()));
    assert!(clients.contains(&"claude_code".to_string()));
    assert_eq!(clients.len(), 2);

    let caps = named_agent_host_capabilities();
    assert_eq!(caps["version"], 1);
    assert_eq!(caps["l0_manual_report"], "manual_report");
    assert_eq!(
        caps["subtask_parallelism"]["parent_exit_autocompletes_children"],
        false
    );
    assert_eq!(
        caps["subtask_parallelism"]["parent_rollup_copies_artifacts"],
        false
    );

    let codex = negotiate_named_adapter("codex_cli", &[AdapterCapability::StatusRead], &[]);
    assert_eq!(codex.decision, NegotiationDecision::Usable);
    let codex_start = negotiate_named_adapter(
        "codex_cli",
        &[AdapterCapability::Start, AdapterCapability::StatusRead],
        &[],
    );
    assert_eq!(
        codex_start.decision,
        NegotiationDecision::HumanContinuationRequired
    );
    assert!(
        codex_start
            .missing_required
            .contains(&AdapterCapability::Start)
    );
    assert!(codex_start.human_continuation.is_some());

    let claude_start = negotiate_named_adapter("claude_code", &[AdapterCapability::Start], &[]);
    assert_eq!(
        claude_start.decision,
        NegotiationDecision::HumanContinuationRequired
    );
    assert!(claude_start.human_continuation.is_some());
}
