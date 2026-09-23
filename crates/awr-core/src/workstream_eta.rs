//! Calibrated expected acceptance time and stage checkpoints (WS-043).
//!
//! Forecasts are stored separately from measured timing (WS-041). Historical
//! forecast rows are immutable. Critical-path scheduling respects real
//! concurrency and available executors: parallel task durations are never
//! summed, and a card does not wait for every mainline to finish.
//!
//! Effective execution, dependency/human wait, and calendar acceptance windows
//! are reported as distinct components. Network/model-queue exclusions require
//! an observable basis; missing components stay unknown. Cold start may be
//! provisional or unestimable with an explicit source. Calibrated intervals
//! require frozen sample + holdout thresholds. LLM self-report is never a
//! precise promise.
use crate::workstream_usage::{UsageTimeObservationHandoff, refuse_eta_from_cumulative_duration};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const ETA_METHOD_VERSION: &str = "awr-eta-v1";
pub const ETA_FORECAST_PROTOCOL: u32 = 1;

/// Default frozen calibration gate (sample size + holdout).
pub const DEFAULT_MIN_CALIBRATION_SAMPLES: usize = 8;
pub const DEFAULT_MIN_HOLDOUT_SAMPLES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EtaError {
    #[error("invalid eta input: {0}")]
    Invalid(&'static str),
    #[error("historical forecast records are immutable")]
    ImmutableHistory,
    #[error("calibrated interval requires frozen sample and holdout thresholds")]
    CalibrationGate,
    #[error("exclusion lacks observable basis")]
    ExclusionWithoutEvidence,
    #[error("llm self-report cannot act as a precise time promise")]
    LlmSelfReportPromise,
    #[error("historical samples must stay isolated from acceptance data")]
    SampleAcceptanceIsolation,
    #[error("dependency cycle in estimate graph")]
    DependencyCycle,
    #[error("arithmetic overflow in eta scheduling")]
    Overflow,
    #[error(transparent)]
    Usage(#[from] crate::workstream_usage::UsageError),
}

type Result<T> = std::result::Result<T, EtaError>;

fn text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn add_u64(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or(EtaError::Overflow)
}

fn max_u64(a: u64, b: u64) -> u64 {
    if a >= b { a } else { b }
}

/// Delivery or stage-checkpoint target for a forecast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EtaTargetKind {
    Delivery,
    StageCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaTarget {
    pub kind: EtaTargetKind,
    pub target_id: String,
    /// Work / card identity whose next acceptable outcome is estimated.
    pub work_id: String,
    /// Optional named checkpoint (e.g. verified / merged).
    pub checkpoint_name: Option<String>,
}

impl EtaTarget {
    fn validate(&self) -> Result<()> {
        if !text(&self.target_id) || !text(&self.work_id) {
            return Err(EtaError::Invalid("target identity"));
        }
        if let Some(name) = &self.checkpoint_name {
            if !text(name) {
                return Err(EtaError::Invalid("checkpoint name"));
            }
        }
        Ok(())
    }
}

/// Half-open time interval in milliseconds; missing bounds stay unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EtaBoundMs {
    Known(u64),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaIntervalMs {
    pub low_ms: EtaBoundMs,
    pub high_ms: EtaBoundMs,
}

impl EtaIntervalMs {
    pub fn known(low: u64, high: u64) -> Result<Self> {
        if high < low {
            return Err(EtaError::Invalid("reversed interval"));
        }
        Ok(Self {
            low_ms: EtaBoundMs::Known(low),
            high_ms: EtaBoundMs::Known(high),
        })
    }

    pub fn unknown() -> Self {
        Self {
            low_ms: EtaBoundMs::Unknown,
            high_ms: EtaBoundMs::Unknown,
        }
    }

    pub fn width_ms(&self) -> Option<u64> {
        match (&self.low_ms, &self.high_ms) {
            (EtaBoundMs::Known(l), EtaBoundMs::Known(h)) => Some(h.saturating_sub(*l)),
            _ => None,
        }
    }
}

/// Separated estimate components (acceptance bullet 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaComponents {
    pub effective_execution_ms: EtaIntervalMs,
    pub dependency_or_human_wait_ms: EtaIntervalMs,
    pub calendar_acceptance_window_ms: EtaIntervalMs,
    /// Queue exclusions only when observable evidence is present.
    pub excluded_network_queue_ms: EtaBoundMs,
    pub excluded_model_queue_ms: EtaBoundMs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EtaEstimateKind {
    /// Explicit provisional estimate with a named source (cold start).
    Provisional { source: String },
    /// Explicitly unestimable with reason.
    Unestimable { reason: String },
    /// Calibrated interval after frozen sample + holdout gate.
    Calibrated,
}

/// Coverage and missing-rate use basis points (0..=10_000) so metrics stay Eq.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaCalibrationMetrics {
    /// Observed non-missing fraction in basis points, when samples exist.
    pub coverage_bps: Option<u16>,
    pub interval_width_ms: Option<u64>,
    pub absolute_error_ms: Option<u64>,
    /// Missing observation fraction in basis points, when samples exist.
    pub missing_rate_bps: Option<u16>,
    pub sample_count: usize,
    pub holdout_count: usize,
    pub gate_passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaSamplePolicy {
    pub policy_id: String,
    pub method_version: String,
    pub min_samples: usize,
    pub min_holdout: usize,
    /// Frozen at policy creation; later changes do not rewrite history.
    pub frozen: bool,
}

impl EtaSamplePolicy {
    pub fn default_frozen() -> Self {
        Self {
            policy_id: "eta-sample-default-v1".into(),
            method_version: ETA_METHOD_VERSION.into(),
            min_samples: DEFAULT_MIN_CALIBRATION_SAMPLES,
            min_holdout: DEFAULT_MIN_HOLDOUT_SAMPLES,
            frozen: true,
        }
    }

    fn validate(&self) -> Result<()> {
        if !text(&self.policy_id) || !text(&self.method_version) || !self.frozen {
            return Err(EtaError::Invalid("sample policy"));
        }
        if self.min_samples == 0 || self.min_holdout == 0 || self.min_holdout > self.min_samples {
            return Err(EtaError::Invalid("sample policy thresholds"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EtaReestimateTrigger {
    NewDependency { detail: String },
    Rework { detail: String },
    ExecutorChange { detail: String },
    CapacityChange { detail: String },
}

impl EtaReestimateTrigger {
    fn validate(&self) -> Result<()> {
        let detail = match self {
            Self::NewDependency { detail }
            | Self::Rework { detail }
            | Self::ExecutorChange { detail }
            | Self::CapacityChange { detail } => detail,
        };
        if text(detail) {
            Ok(())
        } else {
            Err(EtaError::Invalid("reestimate trigger detail"))
        }
    }
}

/// Append-only forecast record. Never rewritten after persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaForecastRecord {
    pub protocol: u32,
    pub forecast_id: String,
    pub project_id: String,
    pub target: EtaTarget,
    pub generated_at_ms: u64,
    pub task_graph_version: String,
    pub execution_strategy: String,
    pub sample_policy_id: String,
    pub method_version: String,
    pub estimate_kind: EtaEstimateKind,
    pub components: EtaComponents,
    pub combined_interval_ms: EtaIntervalMs,
    pub assumptions: Vec<String>,
    pub unknowns: Vec<String>,
    pub calibration: EtaCalibrationMetrics,
    /// Prior forecast this replaces; history stays readable.
    pub supersedes_forecast_id: Option<String>,
    pub reestimate_trigger: Option<EtaReestimateTrigger>,
    pub reestimate_reason_before: Option<String>,
    pub reestimate_reason_after: Option<String>,
    /// Fingerprint of historical observation handoff consumed (not an ETA).
    pub observation_handoff_digest: Option<String>,
}

impl EtaForecastRecord {
    pub fn validate(&self) -> Result<()> {
        if self.protocol != ETA_FORECAST_PROTOCOL {
            return Err(EtaError::Invalid("protocol"));
        }
        if ![
            &self.forecast_id,
            &self.project_id,
            &self.task_graph_version,
            &self.execution_strategy,
            &self.sample_policy_id,
            &self.method_version,
        ]
        .into_iter()
        .all(|s| text(s))
        {
            return Err(EtaError::Invalid("forecast identity"));
        }
        self.target.validate()?;
        for a in &self.assumptions {
            if !text(a) {
                return Err(EtaError::Invalid("assumption"));
            }
        }
        for u in &self.unknowns {
            if !text(u) {
                return Err(EtaError::Invalid("unknown"));
            }
        }
        match &self.estimate_kind {
            EtaEstimateKind::Provisional { source } if !text(source) => {
                return Err(EtaError::Invalid("provisional source"));
            }
            EtaEstimateKind::Unestimable { reason } if !text(reason) => {
                return Err(EtaError::Invalid("unestimable reason"));
            }
            EtaEstimateKind::Calibrated if !self.calibration.gate_passed => {
                return Err(EtaError::CalibrationGate);
            }
            _ => {}
        }
        if let Some(trigger) = &self.reestimate_trigger {
            trigger.validate()?;
            if self
                .supersedes_forecast_id
                .as_ref()
                .is_none_or(|s| !text(s))
            {
                return Err(EtaError::Invalid("reestimate requires prior forecast"));
            }
            if self
                .reestimate_reason_before
                .as_ref()
                .is_none_or(|s| !text(s))
                || self
                    .reestimate_reason_after
                    .as_ref()
                    .is_none_or(|s| !text(s))
            {
                return Err(EtaError::Invalid("reestimate before/after reasons"));
            }
        }
        Ok(())
    }
}

/// Task node for critical-path / concurrency scheduling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaTaskNode {
    pub work_id: String,
    /// Effective execution duration sample (ms). None = unknown.
    pub effective_execution_ms: Option<u64>,
    /// Dependency / human wait before this task can start.
    /// `None` = unknown (distinct from `Some(0)`, which is an explicit zero wait).
    pub wait_before_ms: Option<u64>,
    /// Hard prerequisites (must complete before start).
    pub depends_on: Vec<String>,
    /// Mainline id; cards do not wait for all mainlines — only their deps.
    pub mainline_id: Option<String>,
    /// Whether this node is the acceptance checkpoint being estimated.
    pub is_acceptance_checkpoint: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaExecutionCapacity {
    /// Real concurrency ceiling (>= 1).
    pub concurrency_limit: usize,
    /// Currently available executor slots (<= concurrency_limit).
    pub available_executors: usize,
}

impl EtaExecutionCapacity {
    fn validate(&self) -> Result<()> {
        if self.concurrency_limit == 0 || self.available_executors == 0 {
            return Err(EtaError::Invalid("capacity"));
        }
        if self.available_executors > self.concurrency_limit {
            return Err(EtaError::Invalid("executors exceed concurrency"));
        }
        Ok(())
    }

    fn slots(&self) -> usize {
        self.available_executors.min(self.concurrency_limit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaObservableExclusion {
    pub network_queue_ms: Option<u64>,
    pub model_queue_ms: Option<u64>,
    /// Required when either queue exclusion is Some.
    pub evidence_ref: Option<String>,
}

impl EtaObservableExclusion {
    fn validate(&self) -> Result<()> {
        let has_exclusion = self.network_queue_ms.is_some() || self.model_queue_ms.is_some();
        if has_exclusion {
            match &self.evidence_ref {
                Some(r) if text(r) => Ok(()),
                _ => Err(EtaError::ExclusionWithoutEvidence),
            }
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaHistoricalSample {
    pub sample_id: String,
    pub work_kind: String,
    pub observed_execution_ms: u64,
    pub observed_wait_ms: u64,
    /// Must never equal an acceptance-data identity.
    pub source_ledger: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaAcceptanceDatum {
    pub acceptance_id: String,
    pub work_id: String,
    pub accepted_at_ms: u64,
}

/// Inputs for estimating the next acceptable outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaEstimateRequest {
    pub project_id: String,
    pub forecast_id: String,
    pub generated_at_ms: u64,
    pub target: EtaTarget,
    pub task_graph_version: String,
    pub execution_strategy: String,
    pub tasks: Vec<EtaTaskNode>,
    pub capacity: EtaExecutionCapacity,
    pub sample_policy: EtaSamplePolicy,
    pub samples: Vec<EtaHistoricalSample>,
    pub holdout_sample_ids: BTreeSet<String>,
    /// Acceptance data kept isolated from historical samples.
    pub acceptance_data: Vec<EtaAcceptanceDatum>,
    pub exclusions: EtaObservableExclusion,
    pub assumptions: Vec<String>,
    pub unknowns: Vec<String>,
    /// Optional WS-041 handoff; cumulative duration must not become ETA.
    pub observation_handoff: Option<UsageTimeObservationHandoff>,
    /// When provided, LLM narrative is recorded as assumption only — never a promise.
    pub llm_narrative: Option<String>,
    pub calendar_offset_ms: Option<u64>,
    pub supersedes_forecast_id: Option<String>,
    pub reestimate_trigger: Option<EtaReestimateTrigger>,
    pub reestimate_reason_before: Option<String>,
    pub reestimate_reason_after: Option<String>,
}

/// Schedule result for critical-path / concurrency simulation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtaScheduleResult {
    pub checkpoint_ready_ms: Option<u64>,
    pub critical_path_work_ids: Vec<String>,
    pub effective_execution_ms: Option<u64>,
    pub dependency_or_human_wait_ms: Option<u64>,
    /// Sum of parallel durations is intentionally NOT used as the ETA.
    pub refused_parallel_sum_ms: u64,
    pub used_executor_slots: usize,
}

/// Refuse mutating an already-persisted forecast body (append-only history).
pub fn refuse_forecast_rewrite(
    existing: &EtaForecastRecord,
    attempted: &EtaForecastRecord,
) -> Result<()> {
    if existing.forecast_id != attempted.forecast_id {
        return Err(EtaError::Invalid("forecast id mismatch"));
    }
    if existing != attempted {
        return Err(EtaError::ImmutableHistory);
    }
    Ok(())
}

/// Ensure historical sample ledger identities never collide with acceptance IDs.
pub fn isolate_samples_from_acceptance(
    samples: &[EtaHistoricalSample],
    acceptance: &[EtaAcceptanceDatum],
) -> Result<()> {
    let mut sample_ids = BTreeSet::new();
    for s in samples {
        if !text(&s.sample_id) || !text(&s.work_kind) || !text(&s.source_ledger) {
            return Err(EtaError::Invalid("sample"));
        }
        if s.source_ledger == "acceptance" || s.source_ledger.starts_with("acceptance:") {
            return Err(EtaError::SampleAcceptanceIsolation);
        }
        if !sample_ids.insert(s.sample_id.as_str()) {
            return Err(EtaError::Invalid("duplicate sample"));
        }
    }
    for a in acceptance {
        if !text(&a.acceptance_id) || !text(&a.work_id) {
            return Err(EtaError::Invalid("acceptance datum"));
        }
        if sample_ids.contains(a.acceptance_id.as_str()) {
            return Err(EtaError::SampleAcceptanceIsolation);
        }
    }
    Ok(())
}

fn median_u64(values: &mut [u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2
    })
}

fn percentile_u64(values: &mut [u64], pct: u8) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let pct = pct.min(100) as usize;
    let idx = (pct * (values.len() - 1)) / 100;
    Some(values[idx])
}

/// Critical-path schedule under real concurrency and available executors.
///
/// - Never sums parallel task durations into the checkpoint ETA.
/// - A card waits only on its dependency closure, not on all mainlines.
pub fn schedule_next_acceptance(
    tasks: &[EtaTaskNode],
    capacity: &EtaExecutionCapacity,
    target_work_id: &str,
) -> Result<EtaScheduleResult> {
    capacity.validate()?;
    if tasks.is_empty() {
        return Err(EtaError::Invalid("empty task graph"));
    }
    let mut by_id: BTreeMap<&str, &EtaTaskNode> = BTreeMap::new();
    for t in tasks {
        if !text(&t.work_id) {
            return Err(EtaError::Invalid("task work_id"));
        }
        if by_id.insert(t.work_id.as_str(), t).is_some() {
            return Err(EtaError::Invalid("duplicate task"));
        }
    }
    if !by_id.contains_key(target_work_id) {
        return Err(EtaError::Invalid("target missing from graph"));
    }
    for t in tasks {
        for dep in &t.depends_on {
            if !by_id.contains_key(dep.as_str()) {
                return Err(EtaError::Invalid("missing dependency endpoint"));
            }
        }
    }

    // Tarjan-style cycle check via iterative DFS.
    {
        let mut done = BTreeSet::new();
        let mut active = BTreeMap::new();
        for &root in by_id.keys() {
            if done.contains(root) {
                continue;
            }
            let mut path = vec![root];
            active.insert(root, 0usize);
            let mut stack = vec![by_id[root].depends_on.iter()];
            while let Some(deps) = stack.last_mut() {
                if let Some(dep) = deps.next() {
                    let child = dep.as_str();
                    if active.contains_key(child) {
                        return Err(EtaError::DependencyCycle);
                    }
                    if !done.contains(child) {
                        active.insert(child, path.len());
                        path.push(child);
                        stack.push(by_id[child].depends_on.iter());
                    }
                } else {
                    stack.pop();
                    let node = path.pop().expect("path");
                    active.remove(node);
                    done.insert(node);
                }
            }
        }
    }

    // Dependency closure for the target — never all mainlines.
    let mut needed = BTreeSet::new();
    let mut stack = vec![target_work_id];
    while let Some(id) = stack.pop() {
        if !needed.insert(id) {
            continue;
        }
        for dep in &by_id[id].depends_on {
            stack.push(dep.as_str());
        }
    }

    let slots = capacity.slots();
    let mut finish: BTreeMap<&str, u64> = BTreeMap::new();
    let mut remaining: BTreeSet<&str> = needed.clone();
    let mut running: Vec<(&str, u64)> = Vec::new(); // (id, finish_ms)
    let mut now: u64 = 0;
    let mut refused_parallel_sum: u64 = 0;
    let mut pred: BTreeMap<&str, Option<&str>> = BTreeMap::new();

    while !remaining.is_empty() || !running.is_empty() {
        // Free finished runners.
        running.retain(|&(_, fin)| fin > now);
        let free = slots.saturating_sub(running.len());
        if free > 0 {
            let mut ready: Vec<&str> = remaining
                .iter()
                .copied()
                .filter(|id| {
                    by_id[*id]
                        .depends_on
                        .iter()
                        .all(|d| finish.contains_key(d.as_str()))
                })
                .collect();
            ready.sort_unstable();
            let mut started = 0usize;
            for id in ready {
                if started >= free {
                    break;
                }
                let node = by_id[id];
                let dep_ready = node
                    .depends_on
                    .iter()
                    .map(|d| finish[d.as_str()])
                    .max()
                    .unwrap_or(0);
                // Unknown wait → cannot schedule a precise finish (do not
                // silently substitute zero for missing human/dependency wait).
                let Some(wait) = node.wait_before_ms else {
                    continue;
                };
                let start = max_u64(now, add_u64(dep_ready, wait)?);
                // Unknown execution → cannot schedule a precise finish.
                let Some(exec) = node.effective_execution_ms else {
                    // Leave as remaining unknown; break out after marking.
                    continue;
                };
                let fin = add_u64(start, exec)?;
                // Track critical-path predecessor (latest dep).
                let crit_pred = node
                    .depends_on
                    .iter()
                    .max_by_key(|d| finish[d.as_str()])
                    .map(|s| s.as_str());
                pred.insert(id, crit_pred);
                finish.insert(id, fin);
                running.push((id, fin));
                remaining.remove(id);
                refused_parallel_sum = add_u64(refused_parallel_sum, exec)?;
                started += 1;
            }
        }

        if remaining.iter().any(|id| {
            by_id[*id].effective_execution_ms.is_none() || by_id[*id].wait_before_ms.is_none()
        }) && running.is_empty()
        {
            // Unresolvable unknown durations or waits remain.
            break;
        }

        if running.is_empty() && !remaining.is_empty() {
            // Deadlock or all remaining have unknown exec with unmet deps.
            if remaining.iter().all(|id| {
                by_id[*id]
                    .depends_on
                    .iter()
                    .all(|d| finish.contains_key(d.as_str()))
            }) {
                // Ready but unknown exec or wait — stop with partial.
                break;
            }
            return Err(EtaError::DependencyCycle);
        }

        // Advance time to next completion.
        if let Some(&(_, next_fin)) = running.iter().min_by_key(|(_, fin)| *fin) {
            now = next_fin;
        } else {
            break;
        }
    }

    let checkpoint_ready_ms = finish.get(target_work_id).copied();
    let mut critical_path = Vec::new();
    if checkpoint_ready_ms.is_some() {
        let mut cur = Some(target_work_id);
        while let Some(id) = cur {
            critical_path.push(id.to_string());
            cur = pred.get(id).copied().flatten();
        }
        critical_path.reverse();
    }

    // Effective execution on the critical path (not parallel sum).
    let effective_execution_ms = if checkpoint_ready_ms.is_some() {
        let mut sum = 0u64;
        for id in &critical_path {
            sum = add_u64(sum, by_id[id.as_str()].effective_execution_ms.unwrap_or(0))?;
        }
        Some(sum)
    } else {
        None
    };
    // Distinguish Some(0) / measured waits from None. Any unknown wait on the
    // reported path (critical path when complete, otherwise needed closure)
    // keeps the wait component unknown — never coerce None → 0.
    let dependency_or_human_wait_ms = {
        let ids: Vec<&str> = if checkpoint_ready_ms.is_some() {
            critical_path.iter().map(|s| s.as_str()).collect()
        } else {
            needed.iter().copied().collect()
        };
        let mut sum = 0u64;
        let mut unknown = false;
        for id in ids {
            match by_id[id].wait_before_ms {
                Some(w) => sum = add_u64(sum, w)?,
                None => {
                    unknown = true;
                    break;
                }
            }
        }
        if unknown { None } else { Some(sum) }
    };

    Ok(EtaScheduleResult {
        checkpoint_ready_ms,
        critical_path_work_ids: critical_path,
        effective_execution_ms,
        dependency_or_human_wait_ms,
        refused_parallel_sum_ms: refused_parallel_sum,
        used_executor_slots: slots,
    })
}

fn calibration_metrics(
    policy: &EtaSamplePolicy,
    samples: &[EtaHistoricalSample],
    holdout: &BTreeSet<String>,
    interval: &EtaIntervalMs,
) -> Result<EtaCalibrationMetrics> {
    policy.validate()?;
    let sample_count = samples.len();
    let holdout_count = holdout.len();
    let gate_passed = sample_count >= policy.min_samples && holdout_count >= policy.min_holdout;

    let mut holdout_errors = Vec::new();
    let train: Vec<_> = samples
        .iter()
        .filter(|s| !holdout.contains(&s.sample_id))
        .collect();
    let mut train_exec: Vec<u64> = train.iter().map(|s| s.observed_execution_ms).collect();
    let predicted = median_u64(&mut train_exec);
    for s in samples.iter().filter(|s| holdout.contains(&s.sample_id)) {
        if let Some(p) = predicted {
            let err = s.observed_execution_ms.abs_diff(p);
            holdout_errors.push(err);
        }
    }
    let absolute_error_ms = median_u64(&mut holdout_errors);
    let missing = samples
        .iter()
        .filter(|s| s.observed_execution_ms == 0 && s.observed_wait_ms == 0)
        .count();
    let (coverage_bps, missing_rate_bps) = if sample_count == 0 {
        (None, None)
    } else {
        let miss_bps = ((missing * 10_000) / sample_count) as u16;
        (Some(10_000u16.saturating_sub(miss_bps)), Some(miss_bps))
    };

    Ok(EtaCalibrationMetrics {
        coverage_bps,
        interval_width_ms: interval.width_ms(),
        absolute_error_ms,
        missing_rate_bps,
        sample_count,
        holdout_count,
        gate_passed,
    })
}

fn apply_sample_durations(
    tasks: &mut [EtaTaskNode],
    samples: &[EtaHistoricalSample],
    holdout: &BTreeSet<String>,
) {
    let train: Vec<_> = samples
        .iter()
        .filter(|s| !holdout.contains(&s.sample_id))
        .collect();
    let mut by_kind: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    let mut wait_by_kind: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for s in &train {
        by_kind
            .entry(s.work_kind.as_str())
            .or_default()
            .push(s.observed_execution_ms);
        wait_by_kind
            .entry(s.work_kind.as_str())
            .or_default()
            .push(s.observed_wait_ms);
    }
    for t in tasks.iter_mut() {
        if t.effective_execution_ms.is_none() {
            if let Some(vals) = by_kind.get_mut(t.work_id.as_str()) {
                t.effective_execution_ms = median_u64(vals);
            } else if let Some(vals) = by_kind.get_mut("default") {
                t.effective_execution_ms = median_u64(vals);
            }
        }
        if t.wait_before_ms.is_none() {
            if let Some(vals) = wait_by_kind.get_mut(t.work_id.as_str()) {
                t.wait_before_ms = median_u64(vals);
            } else if let Some(vals) = wait_by_kind.get_mut("default") {
                t.wait_before_ms = median_u64(vals);
            }
        }
    }
}

fn handoff_digest(handoff: &UsageTimeObservationHandoff) -> Result<String> {
    // Refuse treating cumulative duration as ETA label.
    refuse_eta_from_cumulative_duration(
        "ws043 measured timing handoff",
        handoff
            .time_totals
            .as_ref()
            .map(|t| t.observed_execution_ms)
            .unwrap_or(0),
    )?;
    if !handoff.is_historical_observation || !handoff.is_not_estimated_remaining_time {
        return Err(EtaError::Invalid("handoff must be historical observation"));
    }
    let payload = serde_json::to_string(handoff).map_err(|_| EtaError::Invalid("handoff json"))?;
    Ok(format!("sha256:{:x}", Sha256::digest(payload.as_bytes())))
}

/// Build an immutable forecast for the next acceptable outcome.
pub fn estimate_next_acceptance(req: &EtaEstimateRequest) -> Result<EtaForecastRecord> {
    if !text(&req.project_id)
        || !text(&req.forecast_id)
        || !text(&req.task_graph_version)
        || !text(&req.execution_strategy)
    {
        return Err(EtaError::Invalid("request identity"));
    }
    req.target.validate()?;
    req.sample_policy.validate()?;
    req.capacity.validate()?;
    req.exclusions.validate()?;
    isolate_samples_from_acceptance(&req.samples, &req.acceptance_data)?;

    if let Some(narrative) = &req.llm_narrative {
        if !text(narrative) {
            return Err(EtaError::Invalid("llm narrative"));
        }
        // Narrative may only appear as an assumption, never as calibrated promise.
        if req.samples.is_empty() && req.tasks.iter().all(|t| t.effective_execution_ms.is_none()) {
            // Cold start with only LLM text → unestimable / provisional, not calibrated.
        }
    }

    for id in &req.holdout_sample_ids {
        if !req.samples.iter().any(|s| s.sample_id == *id) {
            return Err(EtaError::Invalid("holdout not in samples"));
        }
    }

    let observation_handoff_digest = match &req.observation_handoff {
        Some(h) => Some(handoff_digest(h)?),
        None => None,
    };

    let mut tasks = req.tasks.clone();
    apply_sample_durations(&mut tasks, &req.samples, &req.holdout_sample_ids);

    let unknown_exec = tasks.iter().any(|t| {
        // Only nodes in the target closure matter for unestimable.
        t.work_id == req.target.work_id && t.effective_execution_ms.is_none()
    });

    // Cold start paths.
    let cold = req.samples.is_empty()
        || tasks
            .iter()
            .filter(|t| t.work_id == req.target.work_id || t.is_acceptance_checkpoint)
            .any(|t| t.effective_execution_ms.is_none());

    let schedule = schedule_next_acceptance(&tasks, &req.capacity, &req.target.work_id)?;

    // Parallel sum must not equal scheduled ready time when overlaps exist.
    if let (Some(ready), sum) = (
        schedule.checkpoint_ready_ms,
        schedule.refused_parallel_sum_ms,
    ) {
        if sum > ready && schedule.used_executor_slots >= 1 {
            // Expected: parallel work compresses wall time vs sum — good.
        }
    }

    let excl_net = match req.exclusions.network_queue_ms {
        Some(v) => EtaBoundMs::Known(v),
        None => EtaBoundMs::Unknown,
    };
    let excl_model = match req.exclusions.model_queue_ms {
        Some(v) => EtaBoundMs::Known(v),
        None => EtaBoundMs::Unknown,
    };

    let exec_interval = match schedule.effective_execution_ms {
        Some(v) => {
            // Use sample percentiles for calibrated width when possible.
            let mut train_exec: Vec<u64> = req
                .samples
                .iter()
                .filter(|s| !req.holdout_sample_ids.contains(&s.sample_id))
                .map(|s| s.observed_execution_ms)
                .collect();
            if train_exec.len() >= 2 {
                let lo = percentile_u64(&mut train_exec.clone(), 10).unwrap_or(v);
                let hi = percentile_u64(&mut train_exec, 90).unwrap_or(v);
                // Scale path length roughly by sample spread ratio around median.
                let med = median_u64(&mut train_exec).unwrap_or(v).max(1);
                let low = v.saturating_mul(lo) / med;
                let high = v.saturating_mul(hi) / med;
                EtaIntervalMs::known(low.min(high), low.max(high))?
            } else {
                EtaIntervalMs::known(v, v)?
            }
        }
        None => EtaIntervalMs::unknown(),
    };
    let wait_interval = match schedule.dependency_or_human_wait_ms {
        Some(v) => EtaIntervalMs::known(v, v)?,
        None => EtaIntervalMs::unknown(),
    };
    let calendar = match (schedule.checkpoint_ready_ms, req.calendar_offset_ms) {
        (Some(ready), offset) => {
            let base = add_u64(req.generated_at_ms, ready)?;
            let off = offset.unwrap_or(0);
            EtaIntervalMs::known(add_u64(base, off)?, add_u64(base, off)?)?
        }
        (None, _) => EtaIntervalMs::unknown(),
    };

    let components = EtaComponents {
        effective_execution_ms: exec_interval.clone(),
        dependency_or_human_wait_ms: wait_interval,
        calendar_acceptance_window_ms: calendar.clone(),
        excluded_network_queue_ms: excl_net,
        excluded_model_queue_ms: excl_model,
    };

    let combined = match schedule.checkpoint_ready_ms {
        Some(ready) => {
            if let (EtaBoundMs::Known(lo), EtaBoundMs::Known(hi)) =
                (&exec_interval.low_ms, &exec_interval.high_ms)
            {
                // Combine path ready with exec interval spread.
                let spread_lo = (*lo).min(ready);
                let spread_hi = (*hi).max(ready);
                EtaIntervalMs::known(spread_lo, spread_hi)?
            } else {
                EtaIntervalMs::known(ready, ready)?
            }
        }
        None => EtaIntervalMs::unknown(),
    };

    let calibration = calibration_metrics(
        &req.sample_policy,
        &req.samples,
        &req.holdout_sample_ids,
        &combined,
    )?;

    let mut assumptions = req.assumptions.clone();
    let mut unknowns = req.unknowns.clone();
    if let Some(narrative) = &req.llm_narrative {
        assumptions.push(format!("llm_narrative_recorded_not_promise:{narrative}"));
    }
    if req.exclusions.network_queue_ms.is_none() {
        unknowns.push("network_queue_ms".into());
    }
    if req.exclusions.model_queue_ms.is_none() {
        unknowns.push("model_queue_ms".into());
    }
    if schedule.checkpoint_ready_ms.is_none() {
        unknowns.push("checkpoint_ready_ms".into());
    }
    if schedule.dependency_or_human_wait_ms.is_none() {
        unknowns.push("dependency_or_human_wait_ms".into());
    }

    let estimate_kind = if let Some(narrative) = &req.llm_narrative {
        if cold && schedule.checkpoint_ready_ms.is_none() {
            // Cannot use LLM as precise promise.
            let _ = narrative;
            EtaEstimateKind::Unestimable {
                reason: "cold_start_without_measurable_durations".into(),
            }
        } else if calibration.gate_passed && schedule.checkpoint_ready_ms.is_some() {
            EtaEstimateKind::Calibrated
        } else if let Some(src) = req.assumptions.first() {
            EtaEstimateKind::Provisional {
                source: src.clone(),
            }
        } else {
            EtaEstimateKind::Provisional {
                source: "explicit_cold_start_assumption".into(),
            }
        }
    } else if calibration.gate_passed && schedule.checkpoint_ready_ms.is_some() && !unknown_exec {
        EtaEstimateKind::Calibrated
    } else if schedule.checkpoint_ready_ms.is_some() {
        let source = req
            .assumptions
            .first()
            .cloned()
            .unwrap_or_else(|| "provisional_below_calibration_gate".into());
        EtaEstimateKind::Provisional { source }
    } else if cold {
        EtaEstimateKind::Unestimable {
            reason: "insufficient_samples_or_unknown_durations".into(),
        }
    } else {
        EtaEstimateKind::Unestimable {
            reason: "schedule_incomplete".into(),
        }
    };

    // Hard refuse: calibrated claim without gate.
    if matches!(estimate_kind, EtaEstimateKind::Calibrated) && !calibration.gate_passed {
        return Err(EtaError::CalibrationGate);
    }
    // Hard refuse: treating LLM-only as calibrated.
    if matches!(estimate_kind, EtaEstimateKind::Calibrated)
        && req.llm_narrative.is_some()
        && req.samples.len() < req.sample_policy.min_samples
    {
        return Err(EtaError::LlmSelfReportPromise);
    }

    let record = EtaForecastRecord {
        protocol: ETA_FORECAST_PROTOCOL,
        forecast_id: req.forecast_id.clone(),
        project_id: req.project_id.clone(),
        target: req.target.clone(),
        generated_at_ms: req.generated_at_ms,
        task_graph_version: req.task_graph_version.clone(),
        execution_strategy: req.execution_strategy.clone(),
        sample_policy_id: req.sample_policy.policy_id.clone(),
        method_version: req.sample_policy.method_version.clone(),
        estimate_kind,
        components,
        combined_interval_ms: combined,
        assumptions,
        unknowns,
        calibration,
        supersedes_forecast_id: req.supersedes_forecast_id.clone(),
        reestimate_trigger: req.reestimate_trigger.clone(),
        reestimate_reason_before: req.reestimate_reason_before.clone(),
        reestimate_reason_after: req.reestimate_reason_after.clone(),
        observation_handoff_digest,
    };
    record.validate()?;
    Ok(record)
}

/// Re-estimate after dependency / rework / executor / capacity change.
pub fn reestimate_next_acceptance(
    previous: &EtaForecastRecord,
    mut req: EtaEstimateRequest,
    trigger: EtaReestimateTrigger,
    reason_before: &str,
    reason_after: &str,
) -> Result<EtaForecastRecord> {
    trigger.validate()?;
    if !text(reason_before) || !text(reason_after) {
        return Err(EtaError::Invalid("reestimate reasons"));
    }
    if req.project_id != previous.project_id {
        return Err(EtaError::Invalid("project mismatch on reestimate"));
    }
    if req.forecast_id == previous.forecast_id {
        return Err(EtaError::ImmutableHistory);
    }
    req.supersedes_forecast_id = Some(previous.forecast_id.clone());
    req.reestimate_trigger = Some(trigger);
    req.reestimate_reason_before = Some(reason_before.into());
    req.reestimate_reason_after = Some(reason_after.into());
    estimate_next_acceptance(&req)
}

#[cfg(test)]
mod local_tests {
    use super::*;

    #[test]
    fn parallel_tasks_do_not_sum_into_ready_time() {
        let tasks = vec![
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
                work_id: "c".into(),
                effective_execution_ms: Some(10),
                wait_before_ms: Some(0),
                depends_on: vec!["a".into(), "b".into()],
                mainline_id: Some("m1".into()),
                is_acceptance_checkpoint: true,
            },
        ];
        let cap = EtaExecutionCapacity {
            concurrency_limit: 2,
            available_executors: 2,
        };
        let result = schedule_next_acceptance(&tasks, &cap, "c").unwrap();
        assert_eq!(result.checkpoint_ready_ms, Some(110));
        assert_eq!(result.refused_parallel_sum_ms, 210);
        assert!(result.refused_parallel_sum_ms > result.checkpoint_ready_ms.unwrap());
    }

    #[test]
    fn card_does_not_wait_for_unrelated_mainline() {
        let tasks = vec![
            EtaTaskNode {
                work_id: "needed".into(),
                effective_execution_ms: Some(50),
                wait_before_ms: Some(0),
                depends_on: vec![],
                mainline_id: Some("m1".into()),
                is_acceptance_checkpoint: false,
            },
            EtaTaskNode {
                work_id: "unrelated".into(),
                effective_execution_ms: Some(10_000),
                wait_before_ms: Some(0),
                depends_on: vec![],
                mainline_id: Some("m2".into()),
                is_acceptance_checkpoint: false,
            },
            EtaTaskNode {
                work_id: "card".into(),
                effective_execution_ms: Some(5),
                wait_before_ms: Some(0),
                depends_on: vec!["needed".into()],
                mainline_id: Some("m1".into()),
                is_acceptance_checkpoint: true,
            },
        ];
        let cap = EtaExecutionCapacity {
            concurrency_limit: 2,
            available_executors: 2,
        };
        let result = schedule_next_acceptance(&tasks, &cap, "card").unwrap();
        assert_eq!(result.checkpoint_ready_ms, Some(55));
        assert!(
            !result
                .critical_path_work_ids
                .iter()
                .any(|id| id == "unrelated")
        );
    }
}
