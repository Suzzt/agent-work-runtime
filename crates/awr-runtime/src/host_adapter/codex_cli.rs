//! Codex CLI named controlled adapter (L2 host with optional auto-start).
use super::{
    AdapterActionOutcome, AdapterForensics, AdapterStatus, ExecutionHostAdapter,
    NativeExecutionHandle, supported,
};
use awr_core::{
    AdapterCapability, AdapterCapabilityMatrix, AdapterControlMode, AdapterId,
    ExternalExecutionReport, Result,
};
use std::collections::BTreeSet;
use std::sync::RwLock;

#[derive(Debug, Default)]
struct LiveState {
    handles: BTreeSet<String>,
    reports: Vec<ExternalExecutionReport>,
}

/// Codex CLI adapter. Start is declared and fixture-verified via capability flags
/// without requiring a live Codex binary in unit tests.
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
            capabilities: BTreeSet::from([
                AdapterCapability::Start,
                AdapterCapability::StatusRead,
                AdapterCapability::StopConfirmation,
                AdapterCapability::ReconnectResume,
                AdapterCapability::ResultForensics,
            ]),
            auto_startable: true,
            human_continuation:
                "Codex unavailable. Use L0: awr execution report with ExternalExecutionReport and continue in the Codex terminal manually."
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
        if !state.handles.contains(execution_id) {
            return None;
        }
        let phase = state
            .reports
            .iter()
            .rev()
            .find(|r| r.execution_id.to_string() == execution_id)
            .map(|r| format!("{:?}", r.phase).to_ascii_lowercase())
            .unwrap_or_else(|| "running".into());
        Some(AdapterStatus {
            execution_id: execution_id.into(),
            phase,
            verified: true,
            basis: "codex_cli_adapter_status_read".into(),
            summary: "Codex adapter status from retained handle".into(),
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
            summary: "Codex result forensics from retained reports".into(),
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

    fn start(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        let mut state = self.state.write().expect("codex state");
        if state.handles.contains(&handle.execution_id) {
            return Ok(supported(
                "existing Codex handle retained; no duplicate start",
            ));
        }
        state.handles.insert(handle.execution_id.clone());
        Ok(supported(format!(
            "codex_cli start recorded for operation {} (capability-flagged; live binary optional)",
            handle.operation_key
        )))
    }

    fn status(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        let state = self.state.read().expect("codex state");
        if state.handles.contains(&handle.execution_id) {
            Ok(supported("codex_cli status_read: handle present"))
        } else {
            Ok(supported(
                "codex_cli status_read: no live handle; inspect original execution before retry",
            ))
        }
    }

    fn confirm_stop(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        let mut state = self.state.write().expect("codex state");
        if state.handles.remove(&handle.execution_id) {
            Ok(supported(
                "codex_cli stop_confirmation: handle released (not OS kill)",
            ))
        } else {
            Ok(supported(
                "codex_cli stop_confirmation: no handle; already absent",
            ))
        }
    }

    fn reconnect_resume(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        let mut state = self.state.write().expect("codex state");
        if state.handles.contains(&handle.execution_id) {
            Ok(supported(
                "codex_cli reconnect_resume: rejoined existing native session",
            ))
        } else {
            state.handles.insert(handle.execution_id.clone());
            Ok(supported(
                "codex_cli reconnect_resume: restored handle from original execution identity",
            ))
        }
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
