mod support;
use awr_core::workstream_usage::*;
use awr_core::*;
use support::Fixture;

fn money(n: u64) -> UsageMoney {
    UsageMoney {
        currency: "USD".into(),
        micros: n,
    }
}

fn receipt(project: &str) -> UsageReceipt {
    UsageReceipt {
        receipt_id: "receipt-1".into(),
        project_id: project.into(),
        provider_namespace: "account".into(),
        provider: "provider".into(),
        call_id: "call-1".into(),
        model: "model".into(),
        session_id: "session".into(),
        occurred_at_ms: 10,
        channel: UsageChannel::Model,
        attribution: UsageAttribution {
            work_id: "work-1".into(),
            execution_id: "exec-1".into(),
            workstream_id: Some(Id::from(3u128)),
        },
        tokens: Some(UsageTokens {
            input: 10,
            output: 2,
            cached_input: 1,
        }),
        cost: UsageCost::Actual(money(42)),
    }
}

#[test]
fn sqlite_usage_ingest_replay_bindings_and_handoff() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let project_s = project.to_string();
    let r = receipt(&project_s);
    let (stored, receipt) = f
        .store
        .ingest_usage_receipt(project, "ingest-1", &r)
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(stored.receipt_id, "receipt-1");
    let (_, replay) = f
        .store
        .ingest_usage_receipt(project, "ingest-1", &r)
        .unwrap();
    assert!(replay.replayed);

    let mut compaction = r.clone();
    compaction.receipt_id = "receipt-compaction".into();
    compaction.channel = UsageChannel::Compaction;
    let (again, _) = f
        .store
        .ingest_usage_receipt(project, "ingest-1b", &compaction)
        .unwrap();
    assert_eq!(again.receipt_id, "receipt-1");

    let bindings = f.store.usage_occurrence_bindings(project).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].occurrence_mainline_id, Some(Id::from(3u128)));

    f.store
        .record_usage_execution_interval(
            project,
            "iv-1",
            &UsageExecutionInterval {
                execution_id: "exec-1".into(),
                interval: UsageTimeInterval {
                    start_ms: 0,
                    end_ms: 10,
                },
            },
        )
        .unwrap();
    f.store
        .record_usage_execution_interval(
            project,
            "iv-2",
            &UsageExecutionInterval {
                execution_id: "exec-2".into(),
                interval: UsageTimeInterval {
                    start_ms: 5,
                    end_ms: 12,
                },
            },
        )
        .unwrap();
    let time = f.store.usage_time_totals(project).unwrap().unwrap();
    assert_eq!(time.observed_wall_clock_ms, 12);
    assert_eq!(time.observed_execution_ms, 17);

    let correction = UsageCorrection {
        correction_id: "corr-1".into(),
        project_id: project_s.clone(),
        target_receipt_id: "receipt-1".into(),
        corrected_at_ms: 30,
        reason: "restatement".into(),
        prior_cost: UsageCost::Actual(money(42)),
        new_cost: UsageCost::Actual(money(40)),
        actor: "adapter".into(),
    };
    f.store
        .record_usage_correction(project, "corr-req", &correction)
        .unwrap();
    let raw = f.store.usage_cost_totals(project, false).unwrap();
    assert_eq!(raw.actual_micros["USD"], 42);
    let corrected = f.store.usage_cost_totals(project, true).unwrap();
    assert_eq!(corrected.actual_micros["USD"], 40);

    let handoff = f
        .store
        .usage_observation_handoff(
            project,
            &UsageCoverageObservation {
                observed_calls: 1,
                expected_calls: Some(2),
                observed_time_ms: 12,
                expected_time_ms: Some(20),
            },
        )
        .unwrap();
    assert!(handoff.is_historical_observation);
    assert!(handoff.is_not_estimated_remaining_time);
    assert_eq!(handoff.applied_correction_ids, vec!["corr-1".to_string()]);
    assert!(f.store.doctor().unwrap().ok);
    assert_eq!(
        f.store.doctor().unwrap().schema_version,
        awr_store::SCHEMA_VERSION
    );
}
