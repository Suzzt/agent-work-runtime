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

#[test]
fn replay_conflict_corrected_allocation_and_counter_bounds() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let project_s = project.to_string();
    let r = receipt(&project_s);
    f.store
        .ingest_usage_receipt(project, "ingest-1", &r)
        .unwrap();
    let mut other = r.clone();
    other.cost = UsageCost::Actual(money(99));
    assert!(matches!(
        f.store.ingest_usage_receipt(project, "ingest-1", &other),
        Err(Error::RuleViolation(_))
    ));

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
    let mut second = correction.clone();
    second.correction_id = "corr-2".into();
    second.prior_cost = UsageCost::Actual(money(40));
    second.new_cost = UsageCost::Actual(money(10));
    assert!(matches!(
        f.store
            .record_usage_correction(project, "corr-req-2", &second),
        Err(Error::RuleViolation(_))
    ));

    let stale = UsageAllocationRecord {
        allocation_id: "alloc-stale".into(),
        project_id: project_s.clone(),
        receipt_id: "receipt-1".into(),
        rule: Some("split-v1".into()),
        shares: vec![UsageAllocation {
            workstream_id: Some(Id::from(3u128)),
            micros: 42,
        }],
        recorded_at_ms: 31,
    };
    assert!(matches!(
        f.store
            .record_usage_allocation(project, "alloc-stale", &stale),
        Err(Error::InvalidInput(_))
    ));
    let corrected = UsageAllocationRecord {
        shares: vec![UsageAllocation {
            workstream_id: Some(Id::from(3u128)),
            micros: 40,
        }],
        ..stale
    };
    f.store
        .record_usage_allocation(project, "alloc-ok", &corrected)
        .unwrap();
    let mut again = corrected.clone();
    again.allocation_id = "alloc-2".into();
    assert!(matches!(
        f.store.record_usage_allocation(project, "alloc-2", &again),
        Err(Error::RuleViolation(_))
    ));

    let scope = UsageCounterScope {
        project_id: project_s,
        provider_namespace: "account".into(),
        provider: "provider".into(),
        model: "model".into(),
        session_id: "session".into(),
        counter_epoch: "epoch".into(),
    };
    let bad = UsageCounterSnapshot {
        scope: scope.clone(),
        observed_at_ms: 1,
        tokens: UsageTokens {
            input: 1,
            output: 0,
            cached_input: 2,
        },
    };
    assert!(matches!(
        f.store
            .record_usage_counter_snapshot(project, "ctr-bad", &bad),
        Err(Error::InvalidInput(_))
    ));
    let first = UsageCounterSnapshot {
        tokens: UsageTokens {
            input: 10,
            output: 2,
            cached_input: 1,
        },
        ..bad
    };
    f.store
        .record_usage_counter_snapshot(project, "ctr-1", &first)
        .unwrap();
    let decreased = UsageCounterSnapshot {
        observed_at_ms: 2,
        tokens: UsageTokens {
            input: 9,
            output: 2,
            cached_input: 1,
        },
        ..first.clone()
    };
    assert!(matches!(
        f.store
            .record_usage_counter_snapshot(project, "ctr-down", &decreased),
        Err(Error::InvalidInput(_))
    ));
    let mut replay = first.clone();
    replay.tokens.input = 11;
    assert!(matches!(
        f.store
            .record_usage_counter_snapshot(project, "ctr-1", &replay),
        Err(Error::RuleViolation(_))
    ));
}
