//! Acceptance coverage for AWR-WS-043 calibrated ETA + stage checkpoints.
use awr_core::workstream_eta::*;
use awr_core::workstream_usage::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

fn target() -> EtaTarget {
    EtaTarget {
        kind: EtaTargetKind::StageCheckpoint,
        target_id: "cp-verified".into(),
        work_id: "card".into(),
        checkpoint_name: Some("verified".into()),
    }
}

fn capacity(slots: usize) -> EtaExecutionCapacity {
    EtaExecutionCapacity {
        concurrency_limit: slots.max(1),
        available_executors: slots.max(1),
    }
}

fn samples_calibrated() -> (Vec<EtaHistoricalSample>, BTreeSet<String>) {
    let mut samples = Vec::new();
    for i in 0..10 {
        samples.push(EtaHistoricalSample {
            sample_id: format!("s{i}"),
            work_kind: "default".into(),
            observed_execution_ms: 40 + (i as u64 % 5) * 2,
            observed_wait_ms: 2,
            source_ledger: "historical_observation".into(),
        });
    }
    let holdout = ["s7", "s8", "s9"].into_iter().map(str::to_string).collect();
    (samples, holdout)
}

fn base_tasks() -> Vec<EtaTaskNode> {
    vec![
        EtaTaskNode {
            work_id: "a".into(),
            effective_execution_ms: Some(100),
            wait_before_ms: Some(0),
            depends_on: vec![],
            mainline_id: Some("m1".into()),
            is_acceptance_checkpoint: false,
        },
        EtaTaskNode {
            work_id: "b".into(),
            effective_execution_ms: Some(100),
            wait_before_ms: Some(0),
            depends_on: vec![],
            mainline_id: Some("m2".into()),
            is_acceptance_checkpoint: false,
        },
        EtaTaskNode {
            work_id: "card".into(),
            effective_execution_ms: Some(10),
            wait_before_ms: Some(5),
            depends_on: vec!["a".into()],
            mainline_id: Some("m1".into()),
            is_acceptance_checkpoint: true,
        },
    ]
}

fn handoff() -> UsageTimeObservationHandoff {
    UsageTimeObservationHandoff {
        project_id: "proj".into(),
        cost_totals: UsageCostTotals::default(),
        time_totals: Some(UsageTimeTotals {
            observed_wall_clock_ms: 50,
            observed_execution_ms: 80,
        }),
        coverage: UsageCoverageReport {
            observed_calls: 1,
            expected_calls: Some(2),
            observed_time_ms: 50,
            expected_time_ms: Some(100),
            call_coverage: Some((1, 2)),
            time_coverage: Some((50, 100)),
            coverage_is_not_a_cost_column: true,
        },
        applied_correction_ids: vec![],
        is_historical_observation: true,
        is_not_estimated_remaining_time: true,
        wall_clock_distinct_from_execution_sum: true,
    }
}

/// Bullet 1: forecasts separate from measured timing; immutable history fields.
#[test]
fn forecast_record_captures_required_fields_and_refuses_rewrite() {
    let (samples, holdout) = samples_calibrated();
    let req = EtaEstimateRequest {
        project_id: "proj".into(),
        forecast_id: "f-1".into(),
        generated_at_ms: 1_700_000_000_000,
        target: target(),
        task_graph_version: "graph-sha-abc".into(),
        execution_strategy: "critical_path_concurrency_v1".into(),
        tasks: base_tasks(),
        capacity: capacity(2),
        sample_policy: EtaSamplePolicy::default_frozen(),
        samples,
        holdout_sample_ids: holdout,
        acceptance_data: vec![],
        exclusions: EtaObservableExclusion {
            network_queue_ms: Some(3),
            model_queue_ms: None,
            evidence_ref: Some("queue-probe-1".into()),
        },
        assumptions: vec!["stable executor pool".into()],
        unknowns: vec![],
        observation_handoff: Some(handoff()),
        llm_narrative: None,
        calendar_offset_ms: Some(0),
        supersedes_forecast_id: None,
        reestimate_trigger: None,
        reestimate_reason_before: None,
        reestimate_reason_after: None,
    };
    let record = estimate_next_acceptance(&req).unwrap();
    assert_eq!(record.target.target_id, "cp-verified");
    assert_eq!(record.generated_at_ms, 1_700_000_000_000);
    assert_eq!(record.task_graph_version, "graph-sha-abc");
    assert_eq!(record.execution_strategy, "critical_path_concurrency_v1");
    assert_eq!(record.method_version, ETA_METHOD_VERSION);
    assert!(record.observation_handoff_digest.is_some());
    assert!(!record.assumptions.is_empty());
    // network excluded with evidence; model remains unknown
    assert!(matches!(
        record.components.excluded_network_queue_ms,
        EtaBoundMs::Known(3)
    ));
    assert!(matches!(
        record.components.excluded_model_queue_ms,
        EtaBoundMs::Unknown
    ));
    assert!(record.unknowns.iter().any(|u| u == "model_queue_ms"));

    let mut rewritten = record.clone();
    rewritten.generated_at_ms += 1;
    assert!(matches!(
        refuse_forecast_rewrite(&record, &rewritten),
        Err(EtaError::ImmutableHistory)
    ));
    refuse_forecast_rewrite(&record, &record).unwrap();
}

/// Bullet 2: critical path + concurrency; never sum parallels; never wait all mainlines.
#[test]
fn critical_path_concurrency_ignores_unrelated_mainline_and_parallel_sum() {
    let schedule = schedule_next_acceptance(&base_tasks(), &capacity(2), "card").unwrap();
    // card depends only on a (100) + wait 5 + exec 10 = 115; b is unrelated mainline.
    assert_eq!(schedule.checkpoint_ready_ms, Some(115));
    assert!(!schedule.critical_path_work_ids.iter().any(|id| id == "b"));
    // Parallel sum of scheduled nodes includes a+card (and would include b if wrongly required).
    assert!(schedule.refused_parallel_sum_ms >= 110);
    assert_ne!(
        schedule.checkpoint_ready_ms.unwrap(),
        schedule.refused_parallel_sum_ms
    );
}

#[test]
fn parallel_ready_time_is_not_duration_sum() {
    let tasks = vec![
        EtaTaskNode {
            work_id: "a".into(),
            effective_execution_ms: Some(80),
            wait_before_ms: Some(0),
            depends_on: vec![],
            mainline_id: Some("m1".into()),
            is_acceptance_checkpoint: false,
        },
        EtaTaskNode {
            work_id: "b".into(),
            effective_execution_ms: Some(80),
            wait_before_ms: Some(0),
            depends_on: vec![],
            mainline_id: Some("m1".into()),
            is_acceptance_checkpoint: false,
        },
        EtaTaskNode {
            work_id: "card".into(),
            effective_execution_ms: Some(20),
            wait_before_ms: Some(0),
            depends_on: vec!["a".into(), "b".into()],
            mainline_id: Some("m1".into()),
            is_acceptance_checkpoint: true,
        },
    ];
    let with_two = schedule_next_acceptance(&tasks, &capacity(2), "card").unwrap();
    let with_one = schedule_next_acceptance(&tasks, &capacity(1), "card").unwrap();
    assert_eq!(with_two.checkpoint_ready_ms, Some(100)); // max(80,80)+20
    assert_eq!(with_one.checkpoint_ready_ms, Some(180)); // 80+80+20 serialized
    assert_eq!(with_two.refused_parallel_sum_ms, 180);
    assert!(with_two.refused_parallel_sum_ms > with_two.checkpoint_ready_ms.unwrap());
}

/// Bullet 3: separate components; exclusions need evidence; missing stay unknown.
#[test]
fn components_separated_and_exclusions_need_evidence() {
    let bad = EtaObservableExclusion {
        network_queue_ms: Some(9),
        model_queue_ms: None,
        evidence_ref: None,
    };
    let req = EtaEstimateRequest {
        project_id: "proj".into(),
        forecast_id: "f-excl".into(),
        generated_at_ms: 10,
        target: target(),
        task_graph_version: "g1".into(),
        execution_strategy: "s1".into(),
        tasks: base_tasks(),
        capacity: capacity(2),
        sample_policy: EtaSamplePolicy::default_frozen(),
        samples: vec![],
        holdout_sample_ids: BTreeSet::new(),
        acceptance_data: vec![],
        exclusions: bad,
        assumptions: vec!["x".into()],
        unknowns: vec![],
        observation_handoff: None,
        llm_narrative: None,
        calendar_offset_ms: None,
        supersedes_forecast_id: None,
        reestimate_trigger: None,
        reestimate_reason_before: None,
        reestimate_reason_after: None,
    };
    assert!(matches!(
        estimate_next_acceptance(&req),
        Err(EtaError::ExclusionWithoutEvidence)
    ));

    let (samples, holdout) = samples_calibrated();
    let mut ok = req.clone();
    ok.exclusions = EtaObservableExclusion {
        network_queue_ms: Some(9),
        model_queue_ms: None,
        evidence_ref: Some("net-probe".into()),
    };
    ok.samples = samples;
    ok.holdout_sample_ids = holdout;
    let record = estimate_next_acceptance(&ok).unwrap();
    // Three distinct surfaces present.
    assert!(matches!(
        record.components.effective_execution_ms.low_ms,
        EtaBoundMs::Known(_)
    ));
    assert!(matches!(
        record.components.dependency_or_human_wait_ms.low_ms,
        EtaBoundMs::Known(_)
    ));
    assert!(matches!(
        record.components.calendar_acceptance_window_ms.low_ms,
        EtaBoundMs::Known(_)
    ));
    assert!(matches!(
        record.components.excluded_model_queue_ms,
        EtaBoundMs::Unknown
    ));
}

/// Bullet 4: cold start provisional/unestimable; calibration gate; no LLM promise.
#[test]
fn cold_start_and_calibration_gate_and_llm_refusal() {
    let mut tasks = base_tasks();
    for t in &mut tasks {
        t.effective_execution_ms = None;
    }
    let cold = EtaEstimateRequest {
        project_id: "proj".into(),
        forecast_id: "f-cold".into(),
        generated_at_ms: 1,
        target: target(),
        task_graph_version: "g".into(),
        execution_strategy: "s".into(),
        tasks: tasks.clone(),
        capacity: capacity(1),
        sample_policy: EtaSamplePolicy::default_frozen(),
        samples: vec![],
        holdout_sample_ids: BTreeSet::new(),
        acceptance_data: vec![],
        exclusions: EtaObservableExclusion {
            network_queue_ms: None,
            model_queue_ms: None,
            evidence_ref: None,
        },
        assumptions: vec![],
        unknowns: vec![],
        observation_handoff: None,
        llm_narrative: Some("model says two hours".into()),
        calendar_offset_ms: None,
        supersedes_forecast_id: None,
        reestimate_trigger: None,
        reestimate_reason_before: None,
        reestimate_reason_after: None,
    };
    let record = estimate_next_acceptance(&cold).unwrap();
    assert!(matches!(
        record.estimate_kind,
        EtaEstimateKind::Unestimable { .. }
    ));
    assert!(
        record
            .assumptions
            .iter()
            .any(|a| a.starts_with("llm_narrative_recorded_not_promise:"))
    );
    assert!(!record.calibration.gate_passed);

    // Below gate → provisional even with durations.
    let mut provisional = cold.clone();
    provisional.forecast_id = "f-prov".into();
    provisional.tasks = base_tasks();
    provisional.llm_narrative = None;
    provisional.assumptions = vec!["expert-seed".into()];
    provisional.samples = vec![EtaHistoricalSample {
        sample_id: "only".into(),
        work_kind: "default".into(),
        observed_execution_ms: 10,
        observed_wait_ms: 0,
        source_ledger: "seed".into(),
    }];
    provisional.holdout_sample_ids = BTreeSet::new();
    let record = estimate_next_acceptance(&provisional).unwrap();
    assert!(matches!(
        record.estimate_kind,
        EtaEstimateKind::Provisional { .. }
    ));
    assert!(!record.calibration.gate_passed);
    assert!(record.calibration.coverage_bps.is_some());
    assert!(record.calibration.missing_rate_bps.is_some());

    // Gate passed → calibrated; reports metrics.
    let (samples, holdout) = samples_calibrated();
    let mut calibrated = cold.clone();
    calibrated.forecast_id = "f-cal".into();
    calibrated.tasks = base_tasks();
    calibrated.samples = samples;
    calibrated.holdout_sample_ids = holdout;
    calibrated.assumptions = vec!["policy-frozen".into()];
    calibrated.llm_narrative = None;
    let record = estimate_next_acceptance(&calibrated).unwrap();
    assert!(matches!(record.estimate_kind, EtaEstimateKind::Calibrated));
    assert!(record.calibration.gate_passed);
    assert!(record.calibration.interval_width_ms.is_some());
    assert!(record.calibration.sample_count >= DEFAULT_MIN_CALIBRATION_SAMPLES);
    assert!(record.calibration.holdout_count >= DEFAULT_MIN_HOLDOUT_SAMPLES);
}

/// Bullet 5: reestimate triggers with before/after; sample/acceptance isolation.
#[test]
fn reestimate_preserves_history_and_isolates_samples() {
    assert!(matches!(
        isolate_samples_from_acceptance(
            &[EtaHistoricalSample {
                sample_id: "same".into(),
                work_kind: "k".into(),
                observed_execution_ms: 1,
                observed_wait_ms: 0,
                source_ledger: "historical_observation".into(),
            }],
            &[EtaAcceptanceDatum {
                acceptance_id: "same".into(),
                work_id: "w".into(),
                accepted_at_ms: 1,
            }],
        ),
        Err(EtaError::SampleAcceptanceIsolation)
    ));
    assert!(matches!(
        isolate_samples_from_acceptance(
            &[EtaHistoricalSample {
                sample_id: "s1".into(),
                work_kind: "k".into(),
                observed_execution_ms: 1,
                observed_wait_ms: 0,
                source_ledger: "acceptance".into(),
            }],
            &[],
        ),
        Err(EtaError::SampleAcceptanceIsolation)
    ));

    let (samples, holdout) = samples_calibrated();
    let req = EtaEstimateRequest {
        project_id: "proj".into(),
        forecast_id: "f-old".into(),
        generated_at_ms: 100,
        target: target(),
        task_graph_version: "g1".into(),
        execution_strategy: "s1".into(),
        tasks: base_tasks(),
        capacity: capacity(2),
        sample_policy: EtaSamplePolicy::default_frozen(),
        samples: samples.clone(),
        holdout_sample_ids: holdout.clone(),
        acceptance_data: vec![EtaAcceptanceDatum {
            acceptance_id: "acc-1".into(),
            work_id: "other".into(),
            accepted_at_ms: 50,
        }],
        exclusions: EtaObservableExclusion {
            network_queue_ms: None,
            model_queue_ms: None,
            evidence_ref: None,
        },
        assumptions: vec!["baseline".into()],
        unknowns: vec![],
        observation_handoff: None,
        llm_narrative: None,
        calendar_offset_ms: Some(0),
        supersedes_forecast_id: None,
        reestimate_trigger: None,
        reestimate_reason_before: None,
        reestimate_reason_after: None,
    };
    let previous = estimate_next_acceptance(&req).unwrap();
    let mut next = req.clone();
    next.forecast_id = "f-new".into();
    next.generated_at_ms = 200;
    next.task_graph_version = "g2".into();
    next.tasks.push(EtaTaskNode {
        work_id: "new-dep".into(),
        effective_execution_ms: Some(30),
        wait_before_ms: Some(0),
        depends_on: vec![],
        mainline_id: Some("m1".into()),
        is_acceptance_checkpoint: false,
    });
    // card now also depends on new-dep
    next.tasks
        .iter_mut()
        .find(|t| t.work_id == "card")
        .unwrap()
        .depends_on
        .push("new-dep".into());

    let updated = reestimate_next_acceptance(
        &previous,
        next,
        EtaReestimateTrigger::NewDependency {
            detail: "added new-dep".into(),
        },
        "card depended only on a",
        "card depends on a and new-dep",
    )
    .unwrap();
    assert_eq!(updated.supersedes_forecast_id.as_deref(), Some("f-old"));
    assert!(matches!(
        updated.reestimate_trigger,
        Some(EtaReestimateTrigger::NewDependency { .. })
    ));
    assert_eq!(
        updated.reestimate_reason_before.as_deref(),
        Some("card depended only on a")
    );
    assert_eq!(
        updated.reestimate_reason_after.as_deref(),
        Some("card depends on a and new-dep")
    );
    // Same forecast id reuse refused.
    let mut clash = req;
    clash.forecast_id = previous.forecast_id.clone();
    assert!(matches!(
        reestimate_next_acceptance(
            &previous,
            clash,
            EtaReestimateTrigger::Rework {
                detail: "redo".into()
            },
            "before",
            "after",
        ),
        Err(EtaError::ImmutableHistory)
    ));
}

#[test]
fn cumulative_usage_handoff_is_not_eta() {
    let h = handoff();
    assert!(h.is_not_estimated_remaining_time);
    // Building a forecast still records digest without treating duration as remaining ETA.
    let (samples, holdout) = samples_calibrated();
    let req = EtaEstimateRequest {
        project_id: "proj".into(),
        forecast_id: "f-h".into(),
        generated_at_ms: 1,
        target: target(),
        task_graph_version: "g".into(),
        execution_strategy: "s".into(),
        tasks: base_tasks(),
        capacity: capacity(2),
        sample_policy: EtaSamplePolicy::default_frozen(),
        samples,
        holdout_sample_ids: holdout,
        acceptance_data: vec![],
        exclusions: EtaObservableExclusion {
            network_queue_ms: None,
            model_queue_ms: None,
            evidence_ref: None,
        },
        assumptions: vec!["a".into()],
        unknowns: vec![],
        observation_handoff: Some(h.clone()),
        llm_narrative: None,
        calendar_offset_ms: None,
        supersedes_forecast_id: None,
        reestimate_trigger: None,
        reestimate_reason_before: None,
        reestimate_reason_after: None,
    };
    let record = estimate_next_acceptance(&req).unwrap();
    let payload = serde_json::to_string(&h).unwrap();
    let expected = format!("sha256:{:x}", Sha256::digest(payload.as_bytes()));
    assert_eq!(
        record.observation_handoff_digest.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(expected.len(), "sha256:".len() + 64);
}

/// Unknown durations take the `default` sample median. An explicit zero wait
/// stays zero. With no samples, unknown wait is not invented.
#[test]
fn default_samples_fill_unknown_exec_and_wait() {
    let unknown = vec![EtaTaskNode {
        work_id: "card".into(),
        effective_execution_ms: None,
        wait_before_ms: None,
        depends_on: vec![],
        mainline_id: Some("m1".into()),
        is_acceptance_checkpoint: true,
    }];
    let mut req = EtaEstimateRequest {
        project_id: "proj".into(),
        forecast_id: "f-bare".into(),
        generated_at_ms: 1_000,
        target: target(),
        task_graph_version: "g".into(),
        execution_strategy: "s".into(),
        tasks: unknown.clone(),
        capacity: capacity(1),
        sample_policy: EtaSamplePolicy::default_frozen(),
        samples: vec![],
        holdout_sample_ids: BTreeSet::new(),
        acceptance_data: vec![],
        exclusions: EtaObservableExclusion {
            network_queue_ms: None,
            model_queue_ms: None,
            evidence_ref: None,
        },
        assumptions: vec!["seed".into()],
        unknowns: vec![],
        observation_handoff: None,
        llm_narrative: None,
        calendar_offset_ms: Some(0),
        supersedes_forecast_id: None,
        reestimate_trigger: None,
        reestimate_reason_before: None,
        reestimate_reason_after: None,
    };
    let bare = estimate_next_acceptance(&req).unwrap();
    assert!(matches!(
        bare.components.calendar_acceptance_window_ms.low_ms,
        EtaBoundMs::Unknown
    ));

    req.forecast_id = "f-filled".into();
    req.samples = [10, 30, 20]
        .into_iter()
        .zip([4u64, 8, 6])
        .enumerate()
        .map(|(i, (exec, wait))| EtaHistoricalSample {
            sample_id: format!("d{i}"),
            work_kind: "default".into(),
            observed_execution_ms: exec,
            observed_wait_ms: wait,
            source_ledger: "historical_observation".into(),
        })
        .collect();
    let filled = estimate_next_acceptance(&req).unwrap();
    assert!(matches!(
        filled.components.effective_execution_ms.low_ms,
        EtaBoundMs::Known(_)
    ));
    assert!(matches!(
        filled.components.dependency_or_human_wait_ms.low_ms,
        EtaBoundMs::Known(6)
    ));
    assert!(matches!(
        filled.components.calendar_acceptance_window_ms.low_ms,
        EtaBoundMs::Known(1_026)
    ));

    req.forecast_id = "f-explicit-zero".into();
    req.tasks[0].effective_execution_ms = Some(10);
    req.tasks[0].wait_before_ms = Some(0);
    let explicit = estimate_next_acceptance(&req).unwrap();
    assert!(matches!(
        explicit.components.dependency_or_human_wait_ms.low_ms,
        EtaBoundMs::Known(0)
    ));
    assert!(matches!(
        explicit.components.calendar_acceptance_window_ms.low_ms,
        EtaBoundMs::Known(1_010)
    ));
}

/// Unknown human/dependency wait must not be coerced to zero readiness.
#[test]
fn unknown_wait_keeps_acceptance_finish_unknown() {
    let tasks = vec![EtaTaskNode {
        work_id: "card".into(),
        effective_execution_ms: Some(10),
        wait_before_ms: None,
        depends_on: vec![],
        mainline_id: Some("m1".into()),
        is_acceptance_checkpoint: true,
    }];
    let schedule = schedule_next_acceptance(&tasks, &capacity(1), "card").unwrap();
    assert_eq!(
        schedule.checkpoint_ready_ms, None,
        "unknown wait must not yield a concrete finish"
    );
    assert_eq!(
        schedule.dependency_or_human_wait_ms, None,
        "None wait must stay unknown (not Some(0))"
    );

    // Explicit Some(0) remains distinct and schedules normally.
    let known_zero = vec![EtaTaskNode {
        work_id: "card".into(),
        effective_execution_ms: Some(10),
        wait_before_ms: Some(0),
        depends_on: vec![],
        mainline_id: Some("m1".into()),
        is_acceptance_checkpoint: true,
    }];
    let zero = schedule_next_acceptance(&known_zero, &capacity(1), "card").unwrap();
    assert_eq!(zero.checkpoint_ready_ms, Some(10));
    assert_eq!(zero.dependency_or_human_wait_ms, Some(0));

    // Measured wait also remains concrete.
    let measured = vec![EtaTaskNode {
        work_id: "card".into(),
        effective_execution_ms: Some(10),
        wait_before_ms: Some(7),
        depends_on: vec![],
        mainline_id: Some("m1".into()),
        is_acceptance_checkpoint: true,
    }];
    let meas = schedule_next_acceptance(&measured, &capacity(1), "card").unwrap();
    assert_eq!(meas.checkpoint_ready_ms, Some(17));
    assert_eq!(meas.dependency_or_human_wait_ms, Some(7));

    // Unknown wait on a prerequisite keeps the target finish unknown.
    let with_prereq = vec![
        EtaTaskNode {
            work_id: "review".into(),
            effective_execution_ms: Some(20),
            wait_before_ms: None,
            depends_on: vec![],
            mainline_id: Some("m1".into()),
            is_acceptance_checkpoint: false,
        },
        EtaTaskNode {
            work_id: "card".into(),
            effective_execution_ms: Some(10),
            wait_before_ms: Some(0),
            depends_on: vec!["review".into()],
            mainline_id: Some("m1".into()),
            is_acceptance_checkpoint: true,
        },
    ];
    let blocked = schedule_next_acceptance(&with_prereq, &capacity(1), "card").unwrap();
    assert_eq!(blocked.checkpoint_ready_ms, None);
    assert_eq!(blocked.dependency_or_human_wait_ms, None);

    // Forecast path: calendar acceptance stays unknown when wait is unknown.
    let req = EtaEstimateRequest {
        project_id: "proj".into(),
        forecast_id: "f-unknown-wait".into(),
        generated_at_ms: 1_700_000_000_000,
        target: target(),
        task_graph_version: "g-uw".into(),
        execution_strategy: "s".into(),
        tasks,
        capacity: capacity(1),
        sample_policy: EtaSamplePolicy::default_frozen(),
        samples: vec![],
        holdout_sample_ids: BTreeSet::new(),
        acceptance_data: vec![],
        exclusions: EtaObservableExclusion {
            network_queue_ms: None,
            model_queue_ms: None,
            evidence_ref: None,
        },
        assumptions: vec![],
        unknowns: vec![],
        observation_handoff: None,
        llm_narrative: None,
        calendar_offset_ms: Some(0),
        supersedes_forecast_id: None,
        reestimate_trigger: None,
        reestimate_reason_before: None,
        reestimate_reason_after: None,
    };
    let record = estimate_next_acceptance(&req).unwrap();
    assert!(matches!(
        record.components.dependency_or_human_wait_ms.low_ms,
        EtaBoundMs::Unknown
    ));
    assert!(matches!(
        record.components.calendar_acceptance_window_ms.low_ms,
        EtaBoundMs::Unknown
    ));
    assert!(
        record
            .unknowns
            .iter()
            .any(|u| u == "dependency_or_human_wait_ms")
    );
    assert!(record.unknowns.iter().any(|u| u == "checkpoint_ready_ms"));
}
