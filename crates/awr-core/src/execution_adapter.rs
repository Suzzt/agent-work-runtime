//! Execution-adapter capability negotiation (AWR-WS-024).
//!
//! Pure domain types: hosts declare what they can actually do to a coding-agent
//! process. AWR admission / cancel / session-end remain coordination facts and
//! must never be treated as process start or kill.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Stable adapter identity used in negotiation and receipts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AdapterId(pub String);

impl AdapterId {
    pub fn new(id: impl Into<String>) -> crate::Result<Self> {
        let id = id.into();
        if id.trim().is_empty() || id.len() > 128 || id.contains('\0') {
            return Err(crate::Error::InvalidInput(
                "adapter id must be a non-empty bounded string".into(),
            ));
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Independently declared host capabilities. Absence means unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCapability {
    /// Host can launch the named coding-agent client.
    Start,
    /// Host can read live status of an already-known execution.
    StatusRead,
    /// Host can confirm that a stop request was acknowledged (not OS kill).
    StopConfirmation,
    /// Host can reconnect or resume an existing native session.
    ReconnectResume,
    /// Host can collect result forensics (logs, exit, receipt refs).
    ResultForensics,
}

impl AdapterCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::StatusRead => "status_read",
            Self::StopConfirmation => "stop_confirmation",
            Self::ReconnectResume => "reconnect_resume",
            Self::ResultForensics => "result_forensics",
        }
    }

    pub fn all() -> &'static [AdapterCapability] {
        &[
            Self::Start,
            Self::StatusRead,
            Self::StopConfirmation,
            Self::ReconnectResume,
            Self::ResultForensics,
        ]
    }
}

/// How the host participates in execution control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterControlMode {
    /// L0: client/operator reports phases via ExternalExecutionReport.
    ManualReport,
    /// Named controlled adapter with an explicit capability matrix.
    NamedControlled,
}

/// Declared capability matrix for one coding-agent client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterCapabilityMatrix {
    pub adapter_id: AdapterId,
    pub display_name: String,
    pub control_mode: AdapterControlMode,
    pub capabilities: BTreeSet<AdapterCapability>,
    /// When true, process launch may be stubbed in fixtures while still usable.
    pub auto_startable: bool,
    /// Human path used when a requested capability is missing.
    pub human_continuation: String,
}

impl AdapterCapabilityMatrix {
    pub fn supports(&self, capability: AdapterCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self.display_name.trim().is_empty() || self.display_name.len() > 256 {
            return Err(crate::Error::InvalidInput(
                "adapter display_name must be bounded and non-empty".into(),
            ));
        }
        if self.human_continuation.trim().is_empty() || self.human_continuation.len() > 4096 {
            return Err(crate::Error::InvalidInput(
                "adapter human_continuation must be bounded and non-empty".into(),
            ));
        }
        if self.control_mode == AdapterControlMode::ManualReport && self.auto_startable {
            return Err(crate::Error::InvalidInput(
                "manual_report adapters cannot claim auto_startable".into(),
            ));
        }
        // Status read is the minimum useful named-controlled surface.
        if self.control_mode == AdapterControlMode::NamedControlled
            && !self.supports(AdapterCapability::StatusRead)
        {
            return Err(crate::Error::InvalidInput(
                "named controlled adapters must support status_read".into(),
            ));
        }
        Ok(())
    }
}

/// Requested capabilities for negotiation. Unknown IDs are reported, not guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterNegotiationRequest {
    pub adapter_id: AdapterId,
    pub required: BTreeSet<AdapterCapability>,
    pub optional: BTreeSet<AdapterCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NegotiationDecision {
    Usable,
    HumanContinuationRequired,
    UnknownAdapter,
}

/// Result of capability negotiation. Callers must follow human_continuation when set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterNegotiationResult {
    pub adapter_id: AdapterId,
    pub decision: NegotiationDecision,
    pub granted: BTreeSet<AdapterCapability>,
    pub missing_required: BTreeSet<AdapterCapability>,
    pub missing_optional: BTreeSet<AdapterCapability>,
    pub human_continuation: Option<String>,
    /// Explicit reminder: these AWR facts are not process control.
    pub coordination_not_process_control: Vec<String>,
}

pub fn coordination_not_process_control() -> Vec<String> {
    [
        "awr_admission_is_not_process_start",
        "awr_cancel_request_is_not_process_kill",
        "awr_session_end_is_not_process_kill",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub fn negotiate_adapter(
    matrix: Option<&AdapterCapabilityMatrix>,
    request: &AdapterNegotiationRequest,
) -> AdapterNegotiationResult {
    let Some(matrix) = matrix else {
        return AdapterNegotiationResult {
            adapter_id: request.adapter_id.clone(),
            decision: NegotiationDecision::UnknownAdapter,
            granted: BTreeSet::new(),
            missing_required: request.required.clone(),
            missing_optional: request.optional.clone(),
            human_continuation: Some(
                "Unknown adapter. Continue manually via L0 external execution reporting.".into(),
            ),
            coordination_not_process_control: coordination_not_process_control(),
        };
    };

    let granted: BTreeSet<_> = AdapterCapability::all()
        .iter()
        .copied()
        .filter(|c| matrix.supports(*c))
        .collect();
    let missing_required: BTreeSet<_> = request
        .required
        .iter()
        .copied()
        .filter(|c| !matrix.supports(*c))
        .collect();
    let missing_optional: BTreeSet<_> = request
        .optional
        .iter()
        .copied()
        .filter(|c| !matrix.supports(*c))
        .collect();

    let decision = if missing_required.is_empty() {
        NegotiationDecision::Usable
    } else {
        NegotiationDecision::HumanContinuationRequired
    };

    AdapterNegotiationResult {
        adapter_id: request.adapter_id.clone(),
        decision,
        granted,
        missing_required,
        missing_optional,
        human_continuation: if decision == NegotiationDecision::Usable {
            None
        } else {
            Some(matrix.human_continuation.clone())
        },
        coordination_not_process_control: coordination_not_process_control(),
    }
}

/// Resource boundary retained with a subtask claim (WS-021 kinds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtaskResourceBound {
    pub kind: String,
    pub key: String,
    #[serde(default)]
    pub worktree_id: String,
}

impl SubtaskResourceBound {
    pub fn validate(&self) -> crate::Result<()> {
        match self.kind.as_str() {
            "file" | "dir" | "prefix" | "workspace" | "external" | "integration" | "named" => {}
            _ => {
                return Err(crate::Error::InvalidInput(
                    "unsupported subtask resource kind".into(),
                ));
            }
        }
        if self.key.trim().is_empty() || self.key.len() > 2048 || self.key.contains('\0') {
            return Err(crate::Error::InvalidInput(
                "subtask resource key must be bounded and non-empty".into(),
            ));
        }
        if self.worktree_id.len() > 256 || self.worktree_id.contains('\0') {
            return Err(crate::Error::InvalidInput(
                "subtask worktree_id must be bounded".into(),
            ));
        }
        if matches!(self.kind.as_str(), "external" | "integration" | "named")
            && !self.worktree_id.is_empty()
        {
            return Err(crate::Error::InvalidInput(
                "shared resource kinds require empty worktree_id".into(),
            ));
        }
        Ok(())
    }
}

/// Independent child awaiting schedule / handoff / acceptance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtaskIdentity {
    pub work_id: String,
    pub claim_id: String,
    pub parent_session_id: String,
    pub child_session_id: String,
    pub agent_label: String,
    pub resource_bounds: Vec<SubtaskResourceBound>,
    /// Hard dependency work ids that must finish before this subtask may start.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl SubtaskIdentity {
    pub fn validate(&self) -> crate::Result<()> {
        for (label, value) in [
            ("work_id", &self.work_id),
            ("claim_id", &self.claim_id),
            ("parent_session_id", &self.parent_session_id),
            ("child_session_id", &self.child_session_id),
            ("agent_label", &self.agent_label),
        ] {
            if value.trim().is_empty() || value.len() > 128 || value.contains('\0') {
                return Err(crate::Error::InvalidInput(format!(
                    "subtask {label} must be a bounded non-empty identity"
                )));
            }
        }
        if self.child_session_id == self.parent_session_id {
            return Err(crate::Error::InvalidInput(
                "child session must differ from parent session".into(),
            ));
        }
        if self.resource_bounds.is_empty() {
            return Err(crate::Error::InvalidInput(
                "subtask requires at least one resource bound".into(),
            ));
        }
        for bound in &self.resource_bounds {
            bound.validate()?;
        }
        if self.depends_on.len() > 256
            || self
                .depends_on
                .iter()
                .any(|d| d.trim().is_empty() || d.len() > 128)
        {
            return Err(crate::Error::InvalidInput(
                "depends_on entries must be bounded identities".into(),
            ));
        }
        Ok(())
    }
}

/// Parent rollup of child outcomes. Artifacts stay with the child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentRollupRef {
    pub parent_session_id: String,
    pub child_work_id: String,
    pub child_execution_id: String,
    pub child_outcome_ref: String,
    /// Explicit: rollup never embeds child artifact bytes.
    pub artifacts_copied: bool,
}

impl ParentRollupRef {
    pub fn reference(
        parent_session_id: impl Into<String>,
        child_work_id: impl Into<String>,
        child_execution_id: impl Into<String>,
        child_outcome_ref: impl Into<String>,
    ) -> crate::Result<Self> {
        let rollup = Self {
            parent_session_id: parent_session_id.into(),
            child_work_id: child_work_id.into(),
            child_execution_id: child_execution_id.into(),
            child_outcome_ref: child_outcome_ref.into(),
            artifacts_copied: false,
        };
        if rollup.artifacts_copied {
            return Err(crate::Error::InvalidInput(
                "parent rollup must not copy child artifacts".into(),
            ));
        }
        for (label, value) in [
            ("parent_session_id", &rollup.parent_session_id),
            ("child_work_id", &rollup.child_work_id),
            ("child_execution_id", &rollup.child_execution_id),
            ("child_outcome_ref", &rollup.child_outcome_ref),
        ] {
            if value.trim().is_empty() || value.len() > 512 {
                return Err(crate::Error::InvalidInput(format!(
                    "rollup {label} must be bounded and non-empty"
                )));
            }
        }
        Ok(rollup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix(id: &str, caps: &[AdapterCapability], auto: bool) -> AdapterCapabilityMatrix {
        AdapterCapabilityMatrix {
            adapter_id: AdapterId::new(id).unwrap(),
            display_name: id.into(),
            control_mode: AdapterControlMode::NamedControlled,
            capabilities: caps.iter().copied().collect(),
            auto_startable: auto,
            human_continuation: "Continue via L0 manual report.".into(),
        }
    }

    #[test]
    fn negotiation_grants_supported_capabilities() {
        let m = matrix(
            "codex_cli",
            &[
                AdapterCapability::Start,
                AdapterCapability::StatusRead,
                AdapterCapability::ReconnectResume,
                AdapterCapability::ResultForensics,
            ],
            true,
        );
        let req = AdapterNegotiationRequest {
            adapter_id: AdapterId::new("codex_cli").unwrap(),
            required: [AdapterCapability::StatusRead].into_iter().collect(),
            optional: [AdapterCapability::StopConfirmation].into_iter().collect(),
        };
        let result = negotiate_adapter(Some(&m), &req);
        assert_eq!(result.decision, NegotiationDecision::Usable);
        assert!(result.missing_required.is_empty());
        assert!(
            result
                .missing_optional
                .contains(&AdapterCapability::StopConfirmation)
        );
        assert!(result.human_continuation.is_none());
        assert_eq!(result.coordination_not_process_control.len(), 3);
    }

    #[test]
    fn missing_required_forces_human_continuation() {
        let m = matrix("claude_code", &[AdapterCapability::StatusRead], false);
        let req = AdapterNegotiationRequest {
            adapter_id: AdapterId::new("claude_code").unwrap(),
            required: [AdapterCapability::Start].into_iter().collect(),
            optional: BTreeSet::new(),
        };
        let result = negotiate_adapter(Some(&m), &req);
        assert_eq!(
            result.decision,
            NegotiationDecision::HumanContinuationRequired
        );
        assert!(result.human_continuation.is_some());
    }

    #[test]
    fn parent_rollup_never_copies_artifacts() {
        let r = ParentRollupRef::reference("p", "w", "e", "ref://child").unwrap();
        assert!(!r.artifacts_copied);
    }
}
