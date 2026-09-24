mod support;
use awr_core::workstream_eta::*;
use std::collections::BTreeSet;
use support::Fixture;

fn capacity() -> EtaExecutionCapacity {
    EtaExecutionCapacity {
        concurrency_limit: 2,
        available_executors: 2,
    }
}

fn request(project: &str, forecast_id: &str) -> EtaEstimateRequest {
    let mut samples = Vec::new();
    for i in 0..10 {
        samples.push(EtaHistoricalSample {
            sample_id: format!("s{i}"),
            work_kind: "default".into(),
            observed_execution_ms: 40 + i as u64,
            observed_wait_ms: 1,
            source_ledger: "historical_observation".into(),
        });
    }
    let holdout = ["s7", "s8", "s9"]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    EtaEstimateRequest {
        project_id: project.into(),
        forecast_id: forecast_id.into(),
        generated_at_ms: 1_000,
        target: EtaTarget {
            kind: EtaTargetKind::Delivery,
            target_id: "delivery-1".into(),
            work_id: "card".into(),
            checkpoint_name: None,
        },
        task_graph_version: "graph-v1".into(),
        execution_strategy: "critical_path_concurrency_v1".into(),
        tasks: vec![
            EtaTaskNode {
                work_id: "dep".into(),
                effective_execution_ms: Some(50),
                wait_before_ms: Some(0),
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
        capacity: capacity(),
        sample_policy: EtaSamplePolicy::default_frozen(),
        samples,
        holdout_sample_ids: holdout,
        acceptance_data: vec![],
        exclusions: EtaObservableExclusion {
            network_queue_ms: None,
            model_queue_ms: None,
            evidence_ref: None,
        },
        assumptions: vec!["store-test".into()],
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
fn sqlite_eta_append_only_replay_isolation_and_reestimate() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let project_s = project.to_string();

    let sample = EtaHistoricalSample {
        sample_id: "hist-1".into(),
        work_kind: "default".into(),
        observed_execution_ms: 42,
        observed_wait_ms: 3,
        source_ledger: "historical_observation".into(),
    };
    let (_, srec) = f
        .store
        .record_eta_sample(project, "sample-1", &sample)
        .unwrap();
    assert!(!srec.replayed);
    let (_, sreplay) = f
        .store
        .record_eta_sample(project, "sample-1", &sample)
        .unwrap();
    assert!(sreplay.replayed);

    let acceptance = EtaAcceptanceDatum {
        acceptance_id: "acc-1".into(),
        work_id: "card".into(),
        accepted_at_ms: 99,
    };
    f.store
        .record_eta_acceptance(project, "acc-req", &acceptance)
        .unwrap();

    // Isolation: cannot record sample with acceptance id.
    let clash = EtaHistoricalSample {
        sample_id: "acc-1".into(),
        work_kind: "default".into(),
        observed_execution_ms: 1,
        observed_wait_ms: 0,
        source_ledger: "historical_observation".into(),
    };
    assert!(
        f.store
            .record_eta_sample(project, "bad-sample", &clash)
            .is_err()
    );

    let req = request(&project_s, "forecast-1");
    let record = estimate_next_acceptance(&req).unwrap();
    let (stored, receipt) = f
        .store
        .append_eta_forecast(project, "eta-1", &record)
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(stored.forecast_id, "forecast-1");
    let (_, replay) = f
        .store
        .append_eta_forecast(project, "eta-1", &record)
        .unwrap();
    assert!(replay.replayed);

    // Immutable: altered body for same forecast_id is rejected.
    let mut altered = record.clone();
    altered.generated_at_ms += 5;
    assert!(
        f.store
            .append_eta_forecast(project, "eta-1b", &altered)
            .is_err()
    );

    let mut req2 = request(&project_s, "forecast-2");
    req2.capacity.available_executors = 1;
    let next = reestimate_next_acceptance(
        &record,
        req2,
        EtaReestimateTrigger::ExecutorChange {
            detail: "executor replaced".into(),
        },
        "executor A",
        "executor B",
    )
    .unwrap();
    f.store
        .append_eta_forecast(project, "eta-2", &next)
        .unwrap();
    let history = f
        .store
        .list_eta_forecasts_for_target(project, "delivery-1")
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[1].supersedes_forecast_id.as_deref(),
        Some("forecast-1")
    );
    assert_eq!(f.store.list_eta_samples(project).unwrap().len(), 1);
    assert_eq!(f.store.list_eta_acceptance(project).unwrap().len(), 1);
    assert!(f.store.doctor().unwrap().ok);
    assert_eq!(
        f.store.doctor().unwrap().schema_version,
        awr_store::SCHEMA_VERSION
    );
}
