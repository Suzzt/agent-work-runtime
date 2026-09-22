//! Codex CLI named controlled adapter.
//!
//! Until a real host integration supplies attributable start/status/stop
//! receipts, this adapter does **not** advertise auto-start or verified native
//! control from an in-memory handle. Operators continue via L0 reports; status
//! is unknown/unverified unless an ExternalExecutionReport was retained.
use super::{
    AdapterActionOutcome, AdapterForensics, AdapterStatus, ExecutionHostAdapter,
    NativeExecutionHandle, supported, unsupported,
};
use awr_core::{
    AdapterCapability, AdapterCapabilityMatrix, AdapterControlMode, AdapterId,
    ExternalExecutionReport, Result,
};
use std::collections::BTreeSet;
use std::sync::RwLock;

#[derive(Debug, Default)]
struct LiveState {
    /// Coordination-only bookmarks — never evidence of a live Codex process.
    handles: BTreeSet<String>,
    reports: Vec<ExternalExecutionReport>,
}

/// Codex CLI adapter. Native process control is not claimed without host receipts.
pub struct CodexCliAdapter {
    matrix: AdapterCapabilityMatrix,
    state: RwLock<LiveState>,
}

impl CodexCliAdapter {
    pub fn new() -> Self {
        let matrix = AdapterCapabilityMatrix {
            adapter_id: AdapterId::new("codex_cli").expect("static id"),
            display_name: "Codex CLI".into(),
            control_mode: AdapterControlMode::NamedControlled,
            // StatusRead is the NamedControlled minimum and is backed by L0
            // reports only — never by an in-memory handle insert.
            capabilities: BTreeSet::from([
                AdapterCapability::StatusRead,
                AdapterCapability::ResultForensics,
            ]),
            auto_startable: false,
            human_continuation:
                "Codex native auto-start/stop is not integrated yet. Use L0: awr execution report with ExternalExecutionReport and continue in the Codex terminal manually."
                    .into(),
        };
        matrix.validate().expect("codex matrix");
        Self {
            matrix,
            state: RwLock::new(LiveState::default()),
        }
    }

    pub fn observed_status(&self, execution_id: &str) -> Option<AdapterStatus> {
        let state = self.state.read().ok()?;
        if let Some(report) = state
            .reports
            .iter()
            .rev()
            .find(|r| r.execution_id.to_string() == execution_id)
        {
            return Some(AdapterStatus {
                execution_id: execution_id.into(),
                phase: format!("{:?}", report.phase).to_ascii_lowercase(),
                verified: true,
                basis: "codex_cli_l0_external_report".into(),
                summary: "Codex status from retained ExternalExecutionReport".into(),
            });
        }
        if !state.handles.contains(execution_id) {
            return None;
        }
        // In-memory handle alone is coordination bookkeeping, not native observation.
        Some(AdapterStatus {
            execution_id: execution_id.into(),
            phase: "unknown".into(),
            verified: false,
            basis: "codex_cli_in_memory_handle_unverified".into(),
            summary: "No native Codex observation; continue via L0 report or inspect the original execution"
                .into(),
        })
    }

    pub fn forensics_for(&self, execution_id: &str) -> AdapterForensics {
        let state = self.state.read().unwrap();
        let refs: Vec<String> = state
            .reports
            .iter()
            .filter(|r| r.execution_id.to_string() == execution_id)
            .flat_map(|r| r.detail_references.clone())
            .collect();
        AdapterForensics {
            execution_id: execution_id.into(),
            references: refs,
            summary: "Codex result forensics from retained L0 reports".into(),
        }
    }
}

impl Default for CodexCliAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionHostAdapter for CodexCliAdapter {
    fn matrix(&self) -> &AdapterCapabilityMatrix {
        &self.matrix
    }

    fn start(&self, _handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        Ok(unsupported(&self.matrix, AdapterCapability::Start))
    }

    fn status(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        match self.observed_status(&handle.execution_id) {
            Some(status) if status.verified => Ok(supported(format!(
                "codex_cli status_read verified phase={}",
                status.phase
            ))),
            Some(status) => Ok(supported(format!(
                "codex_cli status_read unknown/unverified ({})",
                status.basis
            ))),
            None => Ok(supported(
                "codex_cli status_read: no observation; inspect original execution before retry",
            )),
        }
    }

    fn confirm_stop(&self, _handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        Ok(unsupported(
            &self.matrix,
            AdapterCapability::StopConfirmation,
        ))
    }

    fn reconnect_resume(&self, _handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        // Local handle insert is not native reconnect evidence.
        Ok(unsupported(
            &self.matrix,
            AdapterCapability::ReconnectResume,
        ))
    }

    fn result_forensics(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        let f = self.forensics_for(&handle.execution_id);
        Ok(supported(format!(
            "codex_cli result_forensics: {} refs",
            f.references.len()
        )))
    }

    fn accept_l0_report(&self, report: &ExternalExecutionReport) -> Result<AdapterActionOutcome> {
        report.validate()?;
        let mut state = self.state.write().expect("codex state");
        state.handles.insert(report.execution_id.to_string());
        state.reports.push(report.clone());
        Ok(supported(
            "codex_cli accepted supplemental L0 external report",
        ))
    }
}
