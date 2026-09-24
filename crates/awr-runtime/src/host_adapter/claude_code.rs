//! Claude Code named controlled adapter. Status/reconnect/forensics are supported;
//! auto-start is intentionally not claimed — operators continue via L0 when start
//! is required.
use super::{
    AdapterActionOutcome, ExecutionHostAdapter, NativeExecutionHandle, supported, unsupported,
};
use awr_core::{
    AdapterCapability, AdapterCapabilityMatrix, AdapterControlMode, AdapterId,
    ExternalExecutionReport, Result,
};
use std::collections::BTreeSet;
use std::sync::RwLock;

#[derive(Debug, Default)]
struct LiveState {
    sessions: BTreeSet<String>,
    reports: Vec<ExternalExecutionReport>,
}

pub struct ClaudeCodeAdapter {
    matrix: AdapterCapabilityMatrix,
    state: RwLock<LiveState>,
}

impl ClaudeCodeAdapter {
    pub fn new() -> Self {
        let matrix = AdapterCapabilityMatrix {
            adapter_id: AdapterId::new("claude_code").expect("static id"),
            display_name: "Claude Code".into(),
            control_mode: AdapterControlMode::NamedControlled,
            capabilities: BTreeSet::from([
                AdapterCapability::StatusRead,
                AdapterCapability::ResultForensics,
            ]),
            auto_startable: false,
            human_continuation:
                "Claude Code is not auto-startable from AWR. Start Claude Code yourself, bind with --client generic --external-session claude:<native-id>, and report phases via L0 ExternalExecutionReport."
                    .into(),
        };
        matrix.validate().expect("claude matrix");
        Self {
            matrix,
            state: RwLock::new(LiveState::default()),
        }
    }
}

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionHostAdapter for ClaudeCodeAdapter {
    fn matrix(&self) -> &AdapterCapabilityMatrix {
        &self.matrix
    }

    fn start(&self, _handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        Ok(unsupported(&self.matrix, AdapterCapability::Start))
    }

    fn status(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        let state = self.state.read().expect("claude state");
        let from_report = state
            .reports
            .iter()
            .any(|r| r.execution_id.to_string() == handle.execution_id);
        if from_report {
            Ok(supported(
                "claude_code status_read: verified from retained ExternalExecutionReport",
            ))
        } else {
            // Local session bookmarks alone are unverified.
            Ok(supported(
                "claude_code status_read: unknown/unverified; no L0 report — query original execution before retry",
            ))
        }
    }

    fn confirm_stop(&self, _handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        Ok(unsupported(
            &self.matrix,
            AdapterCapability::StopConfirmation,
        ))
    }

    fn reconnect_resume(&self, _handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        // In-memory session insert is not attributable native reconnect evidence.
        Ok(unsupported(
            &self.matrix,
            AdapterCapability::ReconnectResume,
        ))
    }

    fn result_forensics(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        let state = self.state.read().expect("claude state");
        let n = state
            .reports
            .iter()
            .filter(|r| r.execution_id.to_string() == handle.execution_id)
            .count();
        Ok(supported(format!(
            "claude_code result_forensics: {n} retained reports"
        )))
    }

    fn accept_l0_report(&self, report: &ExternalExecutionReport) -> Result<AdapterActionOutcome> {
        report.validate()?;
        let mut state = self.state.write().expect("claude state");
        state.sessions.insert(report.native_session.clone());
        state.reports.push(report.clone());
        Ok(supported(
            "claude_code accepted L0 manual/client execution report",
        ))
    }
}
