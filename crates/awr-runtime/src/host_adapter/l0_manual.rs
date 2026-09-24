//! L0 manual / client execution reporting adapter.
use super::{
    AdapterActionOutcome, ExecutionHostAdapter, NativeExecutionHandle, supported, unsupported,
};
use awr_core::{
    AdapterCapability, AdapterCapabilityMatrix, AdapterControlMode, AdapterId,
    ExternalExecutionReport, Result,
};
use std::collections::BTreeSet;
use std::sync::RwLock;

pub struct L0ManualAdapter {
    matrix: AdapterCapabilityMatrix,
    reports: RwLock<Vec<ExternalExecutionReport>>,
}

impl L0ManualAdapter {
    pub fn new() -> Self {
        let matrix = AdapterCapabilityMatrix {
            adapter_id: AdapterId::new("manual_report").expect("static id"),
            display_name: "L0 Manual / Client Report".into(),
            control_mode: AdapterControlMode::ManualReport,
            capabilities: BTreeSet::from([
                AdapterCapability::StatusRead,
                AdapterCapability::ResultForensics,
            ]),
            auto_startable: false,
            human_continuation:
                "Use awr execution report with a validated ExternalExecutionReport JSON file. AWR will not start or kill the coding agent."
                    .into(),
        };
        matrix.validate().expect("l0 matrix");
        Self {
            matrix,
            reports: RwLock::new(Vec::new()),
        }
    }

    pub fn reports(&self) -> Vec<ExternalExecutionReport> {
        self.reports.read().expect("l0 reports").clone()
    }
}

impl Default for L0ManualAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionHostAdapter for L0ManualAdapter {
    fn matrix(&self) -> &AdapterCapabilityMatrix {
        &self.matrix
    }

    fn start(&self, _handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        Ok(unsupported(&self.matrix, AdapterCapability::Start))
    }

    fn status(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        let reports = self.reports.read().expect("l0 reports");
        let latest = reports
            .iter()
            .rev()
            .find(|r| r.execution_id.to_string() == handle.execution_id);
        match latest {
            Some(r) => Ok(supported(format!(
                "l0 status_read from report phase {:?}",
                r.phase
            ))),
            None => Ok(supported(
                "l0 status_read: no report yet; wait for client/operator report",
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
        Ok(unsupported(
            &self.matrix,
            AdapterCapability::ReconnectResume,
        ))
    }

    fn result_forensics(&self, handle: &NativeExecutionHandle) -> Result<AdapterActionOutcome> {
        let reports = self.reports.read().expect("l0 reports");
        let n = reports
            .iter()
            .filter(|r| r.execution_id.to_string() == handle.execution_id)
            .count();
        Ok(supported(format!("l0 result_forensics: {n} reports")))
    }

    fn accept_l0_report(&self, report: &ExternalExecutionReport) -> Result<AdapterActionOutcome> {
        report.validate()?;
        self.reports
            .write()
            .expect("l0 reports")
            .push(report.clone());
        Ok(supported("l0 manual report retained"))
    }
}
