//! Append-only persistence for calibrated ETA forecasts (WS-043).
//! Measured usage/time (WS-041) stays in separate tables; forecast history is
//! never rewritten in place.
use crate::{Store, db_error};
use awr_core::workstream_eta::{
    self, EtaAcceptanceDatum, EtaError, EtaEstimateKind, EtaForecastRecord, EtaHistoricalSample,
};
use awr_core::{Error, Id, Result, now_millis};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaIngestReceipt {
    pub request_key: String,
    pub subject_id: String,
    pub op: String,
    pub event_id: String,
    pub replayed: bool,
    pub created_at_ms: i64,
}

fn map_eta(err: EtaError) -> Error {
    match err {
        EtaError::Invalid(msg) => Error::InvalidInput(msg.into()),
        EtaError::ImmutableHistory => {
            Error::RuleViolation("historical forecast records are immutable".into())
        }
        EtaError::CalibrationGate => Error::RuleViolation(
            "calibrated interval requires frozen sample and holdout thresholds".into(),
        ),
        EtaError::ExclusionWithoutEvidence => {
            Error::RuleViolation("exclusion lacks observable basis".into())
        }
        EtaError::LlmSelfReportPromise => {
            Error::RuleViolation("llm self-report cannot act as a precise time promise".into())
        }
        EtaError::SampleAcceptanceIsolation => Error::RuleViolation(
            "historical samples must stay isolated from acceptance data".into(),
        ),
        EtaError::DependencyCycle => Error::RuleViolation("eta dependency cycle".into()),
        EtaError::Overflow => Error::InvalidInput("eta arithmetic overflow".into()),
        EtaError::Usage(u) => Error::RuleViolation(u.to_string()),
    }
}

fn estimate_kind_str(kind: &EtaEstimateKind) -> &'static str {
    match kind {
        EtaEstimateKind::Provisional { .. } => "provisional",
        EtaEstimateKind::Unestimable { .. } => "unestimable",
        EtaEstimateKind::Calibrated => "calibrated",
    }
}

fn load_ingest(
    conn: &rusqlite::Connection,
    project: &str,
    request_key: &str,
    op: &str,
) -> Result<Option<EtaIngestReceipt>> {
    let row = conn
        .query_row(
            "SELECT request_key, subject_id, op, event_id, replayed, created_at_ms
             FROM eta_ingest_receipts WHERE project_id=?1 AND request_key=?2",
            params![project, request_key],
            |r| {
                Ok(EtaIngestReceipt {
                    request_key: r.get(0)?,
                    subject_id: r.get(1)?,
                    op: r.get(2)?,
                    event_id: r.get(3)?,
                    replayed: r.get::<_, i64>(4)? != 0,
                    created_at_ms: r.get(5)?,
                })
            },
        )
        .optional()
        .map_err(db_error)?;
    match row {
        Some(receipt) if receipt.op == op => Ok(Some(receipt)),
        Some(_) => Err(Error::RuleViolation(
            "eta ingest request_key reused with different op".into(),
        )),
        None => Ok(None),
    }
}

fn record_ingest(
    conn: &rusqlite::Connection,
    project: &str,
    request_key: &str,
    subject_id: &str,
    op: &str,
) -> Result<EtaIngestReceipt> {
    let created_at_ms = now_millis()?;
    let event_id = Id::new().to_string();
    conn.execute(
        "INSERT INTO eta_ingest_receipts(
            project_id, request_key, subject_id, op, event_id, replayed, created_at_ms
         ) VALUES (?1,?2,?3,?4,?5,0,?6)",
        params![
            project,
            request_key,
            subject_id,
            op,
            event_id,
            created_at_ms
        ],
    )
    .map_err(db_error)?;
    Ok(EtaIngestReceipt {
        request_key: request_key.into(),
        subject_id: subject_id.into(),
        op: op.into(),
        event_id,
        replayed: false,
        created_at_ms,
    })
}

fn mark_replay(conn: &rusqlite::Connection, project: &str, request_key: &str) -> Result<()> {
    conn.execute(
        "UPDATE eta_ingest_receipts SET replayed=1
         WHERE project_id=?1 AND request_key=?2",
        params![project, request_key],
    )
    .map_err(db_error)?;
    Ok(())
}

fn load_forecast_body(
    conn: &rusqlite::Connection,
    project: &str,
    forecast_id: &str,
) -> Result<Option<EtaForecastRecord>> {
    let body: Option<String> = conn
        .query_row(
            "SELECT body_json FROM eta_forecasts WHERE project_id=?1 AND forecast_id=?2",
            params![project, forecast_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    match body {
        None => Ok(None),
        Some(json) => {
            let record: EtaForecastRecord =
                serde_json::from_str(&json).map_err(|e| Error::Storage(e.to_string()))?;
            record.validate().map_err(map_eta)?;
            Ok(Some(record))
        }
    }
}

impl Store {
    /// Append a forecast. Identical request_key replays; conflicting body for the
    /// same forecast_id is rejected (immutable history).
    pub fn append_eta_forecast(
        &mut self,
        project: Id,
        request_key: &str,
        record: &EtaForecastRecord,
    ) -> Result<(EtaForecastRecord, EtaIngestReceipt)> {
        record.validate().map_err(map_eta)?;
        let project_s = project.to_string();
        if record.project_id != project_s {
            return Err(Error::InvalidInput("forecast project mismatch".into()));
        }
        if request_key.trim().is_empty() {
            return Err(Error::InvalidInput("request_key".into()));
        }
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        if let Some(mut existing_receipt) =
            load_ingest(&tx, &project_s, request_key, "append_forecast")?
        {
            let stored = load_forecast_body(&tx, &project_s, &record.forecast_id)?
                .ok_or_else(|| Error::Storage("eta replay missing forecast body".into()))?;
            workstream_eta::refuse_forecast_rewrite(&stored, record).map_err(map_eta)?;
            mark_replay(&tx, &project_s, request_key)?;
            existing_receipt.replayed = true;
            tx.commit().map_err(db_error)?;
            return Ok((stored, existing_receipt));
        }
        if let Some(existing) = load_forecast_body(&tx, &project_s, &record.forecast_id)? {
            workstream_eta::refuse_forecast_rewrite(&existing, record).map_err(map_eta)?;
            // Same body without matching request_key is still a conflict for a new key.
            return Err(Error::RuleViolation(
                "forecast_id already recorded; history is append-only".into(),
            ));
        }
        if let Some(prev) = &record.supersedes_forecast_id {
            let _ = load_forecast_body(&tx, &project_s, prev)?
                .ok_or_else(|| Error::InvalidInput("supersedes unknown forecast".into()))?;
        }
        let body = serde_json::to_string(record).map_err(|e| Error::Storage(e.to_string()))?;
        let recorded_at_ms = now_millis()?;
        tx.execute(
            "INSERT INTO eta_forecasts(
                project_id, forecast_id, target_id, work_id, generated_at_ms,
                task_graph_version, execution_strategy, sample_policy_id, method_version,
                estimate_kind, supersedes_forecast_id, body_json, recorded_at_ms
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                project_s,
                record.forecast_id,
                record.target.target_id,
                record.target.work_id,
                record.generated_at_ms as i64,
                record.task_graph_version,
                record.execution_strategy,
                record.sample_policy_id,
                record.method_version,
                estimate_kind_str(&record.estimate_kind),
                record.supersedes_forecast_id,
                body,
                recorded_at_ms,
            ],
        )
        .map_err(db_error)?;
        let receipt = record_ingest(
            &tx,
            &project_s,
            request_key,
            &record.forecast_id,
            "append_forecast",
        )?;
        tx.commit().map_err(db_error)?;
        Ok((record.clone(), receipt))
    }

    pub fn get_eta_forecast(
        &self,
        project: Id,
        forecast_id: &str,
    ) -> Result<Option<EtaForecastRecord>> {
        load_forecast_body(&self.conn, &project.to_string(), forecast_id)
    }

    pub fn list_eta_forecasts_for_target(
        &self,
        project: Id,
        target_id: &str,
    ) -> Result<Vec<EtaForecastRecord>> {
        let project_s = project.to_string();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT body_json FROM eta_forecasts
                 WHERE project_id=?1 AND target_id=?2
                 ORDER BY generated_at_ms ASC, forecast_id ASC",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map(params![project_s, target_id], |r| r.get::<_, String>(0))
            .map_err(db_error)?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.map_err(db_error)?;
            let record: EtaForecastRecord =
                serde_json::from_str(&json).map_err(|e| Error::Storage(e.to_string()))?;
            record.validate().map_err(map_eta)?;
            out.push(record);
        }
        Ok(out)
    }

    pub fn record_eta_sample(
        &mut self,
        project: Id,
        request_key: &str,
        sample: &EtaHistoricalSample,
    ) -> Result<(EtaHistoricalSample, EtaIngestReceipt)> {
        let project_s = project.to_string();
        // Isolation against acceptance namespace.
        workstream_eta::isolate_samples_from_acceptance(std::slice::from_ref(sample), &[])
            .map_err(map_eta)?;
        if request_key.trim().is_empty() {
            return Err(Error::InvalidInput("request_key".into()));
        }
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        if let Some(mut existing) = load_ingest(&tx, &project_s, request_key, "record_sample")? {
            let body: String = tx
                .query_row(
                    "SELECT body_json FROM eta_sample_ledger
                     WHERE project_id=?1 AND sample_id=?2",
                    params![project_s, sample.sample_id],
                    |r| r.get(0),
                )
                .map_err(db_error)?;
            let stored: EtaHistoricalSample =
                serde_json::from_str(&body).map_err(|e| Error::Storage(e.to_string()))?;
            if &stored != sample {
                return Err(Error::RuleViolation(
                    "eta sample identity conflict on replay".into(),
                ));
            }
            mark_replay(&tx, &project_s, request_key)?;
            existing.replayed = true;
            tx.commit().map_err(db_error)?;
            return Ok((stored, existing));
        }
        // Reject collision with acceptance ids.
        let acceptance_hit: Option<String> = tx
            .query_row(
                "SELECT acceptance_id FROM eta_acceptance_data
                 WHERE project_id=?1 AND acceptance_id=?2",
                params![project_s, sample.sample_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if acceptance_hit.is_some() {
            return Err(map_eta(EtaError::SampleAcceptanceIsolation));
        }
        let body = serde_json::to_string(sample).map_err(|e| Error::Storage(e.to_string()))?;
        let recorded_at_ms = now_millis()?;
        tx.execute(
            "INSERT INTO eta_sample_ledger(
                project_id, sample_id, work_kind, source_ledger,
                observed_execution_ms, observed_wait_ms, body_json, recorded_at_ms
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                project_s,
                sample.sample_id,
                sample.work_kind,
                sample.source_ledger,
                sample.observed_execution_ms as i64,
                sample.observed_wait_ms as i64,
                body,
                recorded_at_ms,
            ],
        )
        .map_err(db_error)?;
        let receipt = record_ingest(
            &tx,
            &project_s,
            request_key,
            &sample.sample_id,
            "record_sample",
        )?;
        tx.commit().map_err(db_error)?;
        Ok((sample.clone(), receipt))
    }

    pub fn record_eta_acceptance(
        &mut self,
        project: Id,
        request_key: &str,
        datum: &EtaAcceptanceDatum,
    ) -> Result<(EtaAcceptanceDatum, EtaIngestReceipt)> {
        let project_s = project.to_string();
        workstream_eta::isolate_samples_from_acceptance(&[], std::slice::from_ref(datum))
            .map_err(map_eta)?;
        if request_key.trim().is_empty() {
            return Err(Error::InvalidInput("request_key".into()));
        }
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        if let Some(mut existing) = load_ingest(&tx, &project_s, request_key, "record_acceptance")?
        {
            let body: String = tx
                .query_row(
                    "SELECT body_json FROM eta_acceptance_data
                     WHERE project_id=?1 AND acceptance_id=?2",
                    params![project_s, datum.acceptance_id],
                    |r| r.get(0),
                )
                .map_err(db_error)?;
            let stored: EtaAcceptanceDatum =
                serde_json::from_str(&body).map_err(|e| Error::Storage(e.to_string()))?;
            if &stored != datum {
                return Err(Error::RuleViolation(
                    "eta acceptance identity conflict on replay".into(),
                ));
            }
            mark_replay(&tx, &project_s, request_key)?;
            existing.replayed = true;
            tx.commit().map_err(db_error)?;
            return Ok((stored, existing));
        }
        let sample_hit: Option<String> = tx
            .query_row(
                "SELECT sample_id FROM eta_sample_ledger
                 WHERE project_id=?1 AND sample_id=?2",
                params![project_s, datum.acceptance_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if sample_hit.is_some() {
            return Err(map_eta(EtaError::SampleAcceptanceIsolation));
        }
        let body = serde_json::to_string(datum).map_err(|e| Error::Storage(e.to_string()))?;
        let recorded_at_ms = now_millis()?;
        tx.execute(
            "INSERT INTO eta_acceptance_data(
                project_id, acceptance_id, work_id, accepted_at_ms, body_json, recorded_at_ms
             ) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                project_s,
                datum.acceptance_id,
                datum.work_id,
                datum.accepted_at_ms as i64,
                body,
                recorded_at_ms,
            ],
        )
        .map_err(db_error)?;
        let receipt = record_ingest(
            &tx,
            &project_s,
            request_key,
            &datum.acceptance_id,
            "record_acceptance",
        )?;
        tx.commit().map_err(db_error)?;
        Ok((datum.clone(), receipt))
    }

    pub fn list_eta_samples(&self, project: Id) -> Result<Vec<EtaHistoricalSample>> {
        let project_s = project.to_string();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT body_json FROM eta_sample_ledger
                 WHERE project_id=?1 ORDER BY sample_id ASC",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map(params![project_s], |r| r.get::<_, String>(0))
            .map_err(db_error)?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.map_err(db_error)?;
            out.push(serde_json::from_str(&json).map_err(|e| Error::Storage(e.to_string()))?);
        }
        Ok(out)
    }

    pub fn list_eta_acceptance(&self, project: Id) -> Result<Vec<EtaAcceptanceDatum>> {
        let project_s = project.to_string();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT body_json FROM eta_acceptance_data
                 WHERE project_id=?1 ORDER BY acceptance_id ASC",
            )
            .map_err(db_error)?;
        let rows = stmt
            .query_map(params![project_s], |r| r.get::<_, String>(0))
            .map_err(db_error)?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.map_err(db_error)?;
            out.push(serde_json::from_str(&json).map_err(|e| Error::Storage(e.to_string()))?);
        }
        Ok(out)
    }
}
