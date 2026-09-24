//! Command-op → TMCP-010 action mapping owned beside domain command dispatch.
//! The shared decision function lives in `workstream_auth`; this module keeps the
//! command surface matrix explicit for HTTP/MCP/internal callers.
//!
//! TMCP-030 intersects WS-016 agent delegation action sets with these TMCP actions
//! in `crate::delegation_auth` before `authorize_command` runs.

#[cfg(test)]
use crate::workstream_auth::command_authority;
use crate::workstream_auth::command_business_action;

/// Every durable workstream command either maps to a TMCP-010 action or is a
/// special authority (attest / reconcile) outside role templates.
#[cfg_attr(not(test), allow(dead_code))]
pub fn command_action_matrix() -> Vec<(&'static str, Option<&'static str>)> {
    crate::workstream_command::COMMANDS
        .iter()
        .map(|op| {
            let action = command_business_action(op).map(|a| a.as_str());
            (*op, action)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workstream_auth::DomainAuthority;

    #[test]
    fn every_command_has_domain_authority_and_action_or_special() {
        for op in crate::workstream_command::COMMANDS {
            let domain = command_authority(op).expect(op);
            match command_business_action(op) {
                Some(action) => {
                    assert!(
                        !matches!(domain, DomainAuthority::Attest | DomainAuthority::Reconcile),
                        "{op} unexpectedly special while mapped to {}",
                        action.as_str()
                    );
                }
                None => assert!(
                    matches!(domain, DomainAuthority::Attest | DomainAuthority::Reconcile),
                    "{op} missing both business action and special authority"
                ),
            }
        }
        let matrix = command_action_matrix();
        assert_eq!(matrix.len(), crate::workstream_command::COMMANDS.len());
        assert!(matrix.iter().any(|(op, action)| {
            *op == "session.start" && *action == Some("session.maintain_own")
        }));
        assert!(
            matrix
                .iter()
                .any(|(op, action)| { *op == "execution.attest" && action.is_none() })
        );
    }

    #[test]
    fn planning_and_access_ops_are_not_workstream_commands_yet() {
        for op in [
            "planning.publish",
            "planning.edit_draft",
            "access.manage_project",
            "audit.read_project",
        ] {
            assert!(command_authority(op).is_none(), "{op}");
            assert!(command_business_action(op).is_some(), "{op}");
        }
        assert_eq!(
            command_business_action("review.decide").map(|a| a.as_str()),
            Some("review.decide")
        );
        assert!(command_authority("review.decide").is_some());
        assert_eq!(
            command_business_action("delivery.finalize").map(|a| a.as_str()),
            Some("delivery.finalize")
        );
        assert!(command_authority("delivery.finalize").is_some());
    }
}
