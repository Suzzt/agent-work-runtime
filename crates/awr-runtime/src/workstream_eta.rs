//! Runtime estimate / reestimate APIs for calibrated acceptance ETA (WS-043).
//!
//! Consumes WS-041 `UsageTimeObservationHandoff` as historical observation only.
//! Cumulative measured duration is never treated as estimated remaining time.
use awr_core::workstream_eta::{
    self, EtaAcceptanceDatum, EtaError, EtaEstimateRequest, EtaForecastRecord, EtaHistoricalSample,
    EtaReestimateTrigger,
};
use awr_core::workstream_usage::{
    UsageCoverageObservation, UsageTimeObservationHandoff, refuse_eta_from_cumulative_duration,
};

fn refuse_cumulative_as_remaining(
    label: &str,
    ms: u64,
) -> std::result::Result<(), EtaRuntimeError> {
    refuse_eta_from_cumulative_duration(label, ms).map_err(EtaError::from)?;
    Ok(())
}
use awr_core::{Error, Id};
use awr_store::{EtaIngestReceipt, Store};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EtaRuntimeError {
    #[error("project attestation missing for eta operation")]
    ProjectNotAttested,
    #[error("approved project does not match forecast project")]
    ProjectMismatch,
    #[error(transparent)]
    Eta(#[from] EtaError),
    #[error(transparent)]
    Store(#[from] Error),
}

/// Adapter attestation that the project was authorized for ETA ops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestedEtaProject {
    pub project_id: String,
    pub project: Id,
    pub attested: bool,
}

fn assert_attested(project: &AttestedEtaProject) -> std::result::Result<(), EtaRuntimeError> {
    if !project.attested || project.project_id.trim().is_empty() {
        return Err(EtaRuntimeError::ProjectNotAttested);
    }
    if project.project.to_string() != project.project_id {
        return Err(EtaRuntimeError::ProjectMismatch);
    }
    Ok(())
}

/// Optionally attach a WS-041 observation handoff, refusing ETA relabeling.
pub fn attach_observation_handoff(
    req: &mut EtaEstimateRequest,
    handoff: UsageTimeObservationHandoff,
) -> std::result::Result<(), EtaRuntimeError> {
    refuse_cumulative_as_remaining(
        "ws043 measured timing attach",
        handoff
            .time_totals
            .as_ref()
            .map(|t| t.observed_execution_ms)
            .unwrap_or(0),
    )?;
    if !handoff.is_historical_observation || !handoff.is_not_estimated_remaining_time {
        return Err(EtaRuntimeError::Eta(EtaError::Invalid(
            "handoff must be historical observation",
        )));
    }
    req.observation_handoff = Some(handoff);
    Ok(())
}

pub fn estimate_and_persist(
    store: &mut Store,
    project: &AttestedEtaProject,
    request_key: &str,
    req: &EtaEstimateRequest,
) -> std::result::Result<(EtaForecastRecord, EtaIngestReceipt), EtaRuntimeError> {
    assert_attested(project)?;
    if req.project_id != project.project_id {
        return Err(EtaRuntimeError::ProjectMismatch);
    }
    if let Some(handoff) = &req.observation_handoff {
        refuse_cumulative_as_remaining(
            "ws043 measured timing input",
            handoff
                .time_totals
                .as_ref()
                .map(|t| t.observed_execution_ms)
                .unwrap_or(0),
        )?;
    }
    let record = workstream_eta::estimate_next_acceptance(req)?;
    Ok(store.append_eta_forecast(project.project, request_key, &record)?)
}

pub fn reestimate_and_persist(
    store: &mut Store,
    project: &AttestedEtaProject,
    request_key: &str,
    previous_forecast_id: &str,
    req: EtaEstimateRequest,
    trigger: EtaReestimateTrigger,
    reason_before: &str,
    reason_after: &str,
) -> std::result::Result<(EtaForecastRecord, EtaIngestReceipt), EtaRuntimeError> {
    assert_attested(project)?;
    if req.project_id != project.project_id {
        return Err(EtaRuntimeError::ProjectMismatch);
    }
    let previous = store
        .get_eta_forecast(project.project, previous_forecast_id)?
        .ok_or_else(|| Error::InvalidInput("previous forecast not found".into()))?;
    let record = workstream_eta::reestimate_next_acceptance(
        &previous,
        req,
        trigger,
        reason_before,
        reason_after,
    )?;
    Ok(store.append_eta_forecast(project.project, request_key, &record)?)
}

pub fn query_eta_forecast(
    store: &Store,
    project: &AttestedEtaProject,
    forecast_id: &str,
) -> std::result::Result<Option<EtaForecastRecord>, EtaRuntimeError> {
    assert_attested(project)?;
    Ok(store.get_eta_forecast(project.project, forecast_id)?)
}

pub fn query_eta_forecasts_for_target(
    store: &Store,
    project: &AttestedEtaProject,
    target_id: &str,
) -> std::result::Result<Vec<EtaForecastRecord>, EtaRuntimeError> {
    assert_attested(project)?;
    Ok(store.list_eta_forecasts_for_target(project.project, target_id)?)
}

pub fn record_historical_sample(
    store: &mut Store,
    project: &AttestedEtaProject,
    request_key: &str,
    sample: &EtaHistoricalSample,
) -> std::result::Result<(EtaHistoricalSample, EtaIngestReceipt), EtaRuntimeError> {
    assert_attested(project)?;
    Ok(store.record_eta_sample(project.project, request_key, sample)?)
}

pub fn record_acceptance_datum(
    store: &mut Store,
    project: &AttestedEtaProject,
    request_key: &str,
    datum: &EtaAcceptanceDatum,
) -> std::result::Result<(EtaAcceptanceDatum, EtaIngestReceipt), EtaRuntimeError> {
    assert_attested(project)?;
    Ok(store.record_eta_acceptance(project.project, request_key, datum)?)
}

/// Build observation handoff via store (WS-041) for ETA consumers.
pub fn observation_handoff_for_estimate(
    store: &Store,
    project: &AttestedEtaProject,
    coverage: &UsageCoverageObservation,
) -> std::result::Result<UsageTimeObservationHandoff, EtaRuntimeError> {
    assert_attested(project)?;
    let handoff = store.usage_observation_handoff(project.project, coverage)?;
    refuse_cumulative_as_remaining(
        "ws043 measured timing query",
        handoff
            .time_totals
            .as_ref()
            .map(|t| t.observed_execution_ms)
            .unwrap_or(0),
    )?;
    Ok(handoff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::workstream_eta::*;
    use awr_store::SourceRegistration;
    use std::collections::BTreeSet;

    fn fixture() -> (Store, AttestedEtaProject) {
        let root = std::env::temp_dir().join(format!("awr-eta-rt-{}", Id::new()));
        std::fs::create_dir(&root).unwrap();
        let mut store = Store::open(&root.join("state.db")).unwrap();
        let project = store
            .register_project(&root, "eta-example", "ETA Example")
            .unwrap();
        let _ = store
            .register_source(
                project.id,
                &SourceRegistration {
                    domain: "ledger",
                    role: "primary",
                    locator: "file:///example/ledger.yaml",
                    format: "yaml",
                    adapter: "yaml-ledger-v1",
                },
            )
            .unwrap();
        let attested = AttestedEtaProject {
            project_id: project.id.to_string(),
            project: project.id,
            attested: true,
        };
        (store, attested)
    }

    fn base_req(project_id: &str, forecast_id: &str) -> EtaEstimateRequest {
        EtaEstimateRequest {
            project_id: project_id.into(),
            forecast_id: forecast_id.into(),
            generated_at_ms: 1_000,
            target: EtaTarget {
                kind: EtaTargetKind::StageCheckpoint,
                target_id: "cp-1".into(),
                work_id: "card".into(),
                checkpoint_name: Some("verified".into()),
            },
            task_graph_version: "graph-v1".into(),
            execution_strategy: "critical_path_concurrency_v1".into(),
            tasks: vec![
                EtaTaskNode {
                    work_id: "dep".into(),
                    effective_execution_ms: Some(40),
                    wait_before_ms: Some(5),
                    depends_on: vec![],
                    mainline_id: Some("m1".into()),
                    is_acceptance_checkpoint: false,
                },
                EtaTaskNode {
                    work_id: "card".into(),
                    effective_execution_ms: Some(10),
                    wait_before_ms: Some(0),
                    depends_on: vec!["dep".into()],
                    mainline_id: Some("m1".into()),
                    is_acceptance_checkpoint: true,
                },
            ],
            capacity: EtaExecutionCapacity {
                concurrency_limit: 2,
                available_executors: 2,
            },
            sample_policy: EtaSamplePolicy::default_frozen(),
            samples: vec![],
            holdout_sample_ids: BTreeSet::new(),
            acceptance_data: vec![],
            exclusions: EtaObservableExclusion {
                network_queue_ms: None,
                model_queue_ms: None,
                evidence_ref: None,
            },
            assumptions: vec!["unit-test".into()],
            unknowns: vec![],
            observation_handoff: None,
            llm_narrative: None,
            calendar_offset_ms: Some(0),
            supersedes_forecast_id: None,
            reestimate_trigger: None,
            reestimate_reason_before: None,
            reestimate_reason_after: None,
        }
    }

    #[test]
    fn estimate_persist_replay_and_reestimate() {
        let (mut store, project) = fixture();
        let req = base_req(&project.project_id, "f1");
        let (rec, receipt) = estimate_and_persist(&mut store, &project, "est-1", &req).unwrap();
        assert!(!receipt.replayed);
        assert!(matches!(
            rec.estimate_kind,
            EtaEstimateKind::Provisional { .. }
        ));
        let (_, replay) = estimate_and_persist(&mut store, &project, "est-1", &req).unwrap();
        assert!(replay.replayed);

        let mut req2 = base_req(&project.project_id, "f2");
        req2.capacity.available_executors = 1;
        let (re, _) = reestimate_and_persist(
            &mut store,
            &project,
            "est-2",
            "f1",
            req2,
            EtaReestimateTrigger::CapacityChange {
                detail: "executor offline".into(),
            },
            "two executors available",
            "one executor available",
        )
        .unwrap();
        assert_eq!(re.supersedes_forecast_id.as_deref(), Some("f1"));
        let history = query_eta_forecasts_for_target(&store, &project, "cp-1").unwrap();
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn observation_handoff_refuses_eta_label() {
        let (store, project) = fixture();
        let handoff = observation_handoff_for_estimate(
            &store,
            &project,
            &UsageCoverageObservation {
                observed_calls: 0,
                expected_calls: Some(1),
                observed_time_ms: 0,
                expected_time_ms: Some(10),
            },
        )
        .unwrap();
        assert!(handoff.is_historical_observation);
        assert!(handoff.is_not_estimated_remaining_time);
    }
}
