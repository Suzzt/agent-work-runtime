//! Pure accounting over trusted, normalized observations, not a billing collector.
//! Adapters must resolve tenant/account namespaces and stable provider call IDs,
//! authenticate provenance, and persist deduplication atomically. A compaction
//! event ID alone is NOT a call ID; legacy `CompactionUsage` is not a session bill.
//! Missing observations, waiting time and total coverage cannot be inferred here.
use crate::Id;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UsageError {
    #[error("invalid usage input: {0}")]
    Invalid(&'static str),
    #[error("same receipt or call identity has different accounting content")]
    Conflict,
    #[error("cumulative counters cross a scope boundary or move backwards")]
    CounterBoundary,
    #[error("accounting arithmetic overflow")]
    Overflow,
    #[error("cumulative duration must not be presented as estimated remaining time")]
    EtaFromCumulativeDuration,
    #[error("usage correction does not conserve auditable identity")]
    CorrectionAudit,
}
type Result<T> = std::result::Result<T, UsageError>;
fn text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or(UsageError::Overflow)
}

/// Cache tokens are a subset of input tokens, never an additional token charge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageTokens {
    pub input: u64,
    pub output: u64,
    pub cached_input: u64,
}
impl UsageTokens {
    fn validate(self) -> Result<()> {
        if self.cached_input > self.input {
            return Err(UsageError::Invalid("cache exceeds input"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageCounterScope {
    pub project_id: String,
    /// Account/tenant namespace of provider IDs, resolved by the adapter.
    pub provider_namespace: String,
    pub provider: String,
    pub model: String,
    pub session_id: String,
    pub counter_epoch: String,
}
impl UsageCounterScope {
    fn validate(&self) -> Result<()> {
        if [
            &self.project_id,
            &self.provider_namespace,
            &self.provider,
            &self.model,
            &self.session_id,
            &self.counter_epoch,
        ]
        .into_iter()
        .all(|s| text(s))
        {
            Ok(())
        } else {
            Err(UsageError::Invalid("counter scope"))
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageCounterSnapshot {
    pub scope: UsageCounterScope,
    pub observed_at_ms: u64,
    pub tokens: UsageTokens,
}

/// Field checks for one snapshot. A first observation has no previous baseline,
/// so it cannot go through `usage_counter_delta`.
pub fn validate_usage_counter_snapshot(snapshot: &UsageCounterSnapshot) -> Result<()> {
    snapshot.scope.validate()?;
    snapshot.tokens.validate()?;
    Ok(())
}

/// A known baseline is required; a first snapshot is not implicitly zero.
/// Caller must avoid billing this delta again through per-call receipts.
pub fn usage_counter_delta(
    previous: &UsageCounterSnapshot,
    current: &UsageCounterSnapshot,
) -> Result<UsageTokens> {
    validate_usage_counter_snapshot(previous)?;
    validate_usage_counter_snapshot(current)?;
    if previous.scope != current.scope || current.observed_at_ms <= previous.observed_at_ms {
        return Err(UsageError::CounterBoundary);
    }
    let subtract = |a: u64, b: u64| a.checked_sub(b).ok_or(UsageError::CounterBoundary);
    let delta = UsageTokens {
        input: subtract(current.tokens.input, previous.tokens.input)?,
        output: subtract(current.tokens.output, previous.tokens.output)?,
        cached_input: subtract(current.tokens.cached_input, previous.tokens.cached_input)?,
    };
    delta.validate()?;
    Ok(delta)
}

/// Currency units are fixed at one millionth; no implicit FX or rounding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageMoney {
    pub currency: String,
    pub micros: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageCost {
    Actual(UsageMoney),
    ApiEquivalentEstimate {
        amount: UsageMoney,
        pricing_version: String,
    },
    Unknown,
}
impl UsageCost {
    fn amount(&self) -> Option<&UsageMoney> {
        match self {
            Self::Actual(m) | Self::ApiEquivalentEstimate { amount: m, .. } => Some(m),
            Self::Unknown => None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageChannel {
    Model,
    Compaction,
}

/// Historical attribution supplied at occurrence, never looked up from current
/// work ownership. `workstream_id` is the active mainline at occurrence time;
/// None means shared/unallocated and does not mean zero cost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageAttribution {
    /// Task / work item identity at occurrence.
    pub work_id: String,
    /// Execution identity at occurrence.
    pub execution_id: String,
    /// Active mainline (workstream) at occurrence; not current ownership.
    pub workstream_id: Option<Id>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageReceipt {
    pub receipt_id: String,
    pub project_id: String,
    pub provider_namespace: String,
    pub provider: String,
    pub call_id: String,
    pub model: String,
    pub session_id: String,
    pub occurred_at_ms: u64,
    pub channel: UsageChannel,
    pub attribution: UsageAttribution,
    pub tokens: Option<UsageTokens>,
    pub cost: UsageCost,
}
impl UsageReceipt {
    fn validate(&self) -> Result<()> {
        if ![
            &self.receipt_id,
            &self.project_id,
            &self.provider_namespace,
            &self.provider,
            &self.call_id,
            &self.model,
            &self.session_id,
            &self.attribution.work_id,
            &self.attribution.execution_id,
        ]
        .into_iter()
        .all(|s| text(s))
            || self
                .attribution
                .workstream_id
                .is_some_and(|id| u128::from(id) == 0)
        {
            return Err(UsageError::Invalid("receipt identity"));
        }
        if let Some(tokens) = self.tokens {
            tokens.validate()?;
        }
        if let Some(m) = self.cost.amount() {
            if m.currency.len() != 3 || !m.currency.bytes().all(|b| b.is_ascii_uppercase()) {
                return Err(UsageError::Invalid("currency"));
            }
        }
        if let UsageCost::ApiEquivalentEstimate {
            pricing_version, ..
        } = &self.cost
        {
            if !text(pricing_version) {
                return Err(UsageError::Invalid("pricing version"));
            }
        }
        Ok(())
    }
    fn same_call_content(&self, other: &Self) -> bool {
        let mut normalized = other.clone();
        normalized.receipt_id.clone_from(&self.receipt_id);
        normalized.channel = self.channel;
        self == &normalized
    }

    /// Public same-call comparison for store ingest conflict checks.
    pub fn same_call_content_public(&self, other: &Self) -> bool {
        self.same_call_content(other)
    }
}

/// Deduplicate only within one project's supplied batch. Same receipt ID must
/// replay exactly. Different observation IDs/channels may describe the same call,
/// but different content conflicts instead of silently choosing a preferred bill.
/// Provider call IDs must be stable within the account namespace across sessions.
pub fn deduplicate_usage<'a>(
    project: &str,
    receipts: &'a [UsageReceipt],
) -> Result<Vec<&'a UsageReceipt>> {
    if !text(project) {
        return Err(UsageError::Invalid("project"));
    }
    let mut ids = BTreeMap::new();
    let mut calls: BTreeMap<(&str, &str, &str), &UsageReceipt> = BTreeMap::new();
    let mut unique = Vec::new();
    for receipt in receipts {
        receipt.validate()?;
        if receipt.project_id != project {
            return Err(UsageError::Invalid("cross-project receipt"));
        }
        if let Some(old) = ids.insert(receipt.receipt_id.as_str(), receipt) {
            if old != receipt {
                return Err(UsageError::Conflict);
            }
        }
        let key = (
            receipt.provider_namespace.as_str(),
            receipt.provider.as_str(),
            receipt.call_id.as_str(),
        );
        if let Some(old) = calls.get(&key) {
            if !old.same_call_content(receipt) {
                return Err(UsageError::Conflict);
            }
        } else {
            calls.insert(key, receipt);
            unique.push(receipt);
        }
    }
    Ok(unique)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageCostTotals {
    /// Known subtotals only. Never add actual and API-equivalent columns.
    pub actual_micros: BTreeMap<String, u64>,
    pub api_equivalent_micros: BTreeMap<String, u64>,
    pub unknown_calls: usize,
    pub unique_calls: usize,
}
pub fn usage_cost_totals(project: &str, receipts: &[UsageReceipt]) -> Result<UsageCostTotals> {
    let unique = deduplicate_usage(project, receipts)?;
    let mut totals = UsageCostTotals {
        unique_calls: unique.len(),
        ..Default::default()
    };
    for r in unique {
        let (amount, map) = match &r.cost {
            UsageCost::Actual(m) => (m, &mut totals.actual_micros),
            UsageCost::ApiEquivalentEstimate { amount, .. } => {
                (amount, &mut totals.api_equivalent_micros)
            }
            UsageCost::Unknown => {
                totals.unknown_calls += 1;
                continue;
            }
        };
        let entry = map.entry(amount.currency.clone()).or_default();
        *entry = add(*entry, amount.micros)?;
    }
    Ok(totals)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageAllocation {
    pub workstream_id: Option<Id>,
    pub micros: u64,
}
/// Explicit allocations need a durable rule/evidence reference and exact integer
/// conservation. Empty allocations preserve occurrence ownership, including shared
/// unallocated ownership. Unknown costs cannot be numerically allocated.
pub fn allocate_usage_cost(
    receipt: &UsageReceipt,
    allocations: &[UsageAllocation],
    rule: Option<&str>,
) -> Result<Option<Vec<UsageAllocation>>> {
    receipt.validate()?;
    let Some(amount) = receipt.cost.amount() else {
        if !allocations.is_empty() || rule.is_some() {
            return Err(UsageError::Invalid("unknown cost allocation"));
        }
        return Ok(None);
    };
    if allocations.is_empty() {
        if rule.is_some() {
            return Err(UsageError::Invalid("rule without allocation"));
        }
        return Ok(Some(vec![UsageAllocation {
            workstream_id: receipt.attribution.workstream_id,
            micros: amount.micros,
        }]));
    }
    if !rule.is_some_and(text) {
        return Err(UsageError::Invalid("allocation rule required"));
    }
    let mut seen = BTreeSet::new();
    let mut sum = 0;
    for a in allocations {
        if a.workstream_id.is_some_and(|id| u128::from(id) == 0) || !seen.insert(a.workstream_id) {
            return Err(UsageError::Invalid(
                "duplicate or invalid allocation target",
            ));
        }
        sum = add(sum, a.micros)?;
    }
    if sum != amount.micros {
        return Err(UsageError::Invalid("allocation does not conserve cost"));
    }
    Ok(Some(allocations.to_vec()))
}

/// Half-open interval on one common monotonic/normalized clock, milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageTimeInterval {
    pub start_ms: u64,
    pub end_ms: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageTimeTotals {
    pub observed_wall_clock_ms: u64,
    pub observed_execution_ms: u64,
}
/// None means no observations; Some(empty) explicitly observes no execution.
/// Union is observed active wall time, not first-to-last project elapsed time.
/// Execution sum includes parallel execution; caller must deduplicate observations
/// by execution identity before passing intervals. Wait/coverage remain unknown.
pub fn usage_time_totals(
    intervals: Option<&[UsageTimeInterval]>,
) -> Result<Option<UsageTimeTotals>> {
    let Some(intervals) = intervals else {
        return Ok(None);
    };
    let mut sorted = intervals.to_vec();
    let mut execution = 0;
    for interval in &sorted {
        let duration = interval
            .end_ms
            .checked_sub(interval.start_ms)
            .ok_or(UsageError::Invalid("reversed interval"))?;
        execution = add(execution, duration)?;
    }
    sorted.sort_by_key(|i| (i.start_ms, i.end_ms));
    let mut union = 0;
    let mut merged: Option<UsageTimeInterval> = None;
    for interval in sorted {
        match merged.as_mut() {
            Some(current) if interval.start_ms <= current.end_ms => {
                current.end_ms = current.end_ms.max(interval.end_ms)
            }
            Some(current) => {
                union = add(union, current.end_ms - current.start_ms)?;
                *current = interval;
            }
            None => merged = Some(interval),
        }
    }
    if let Some(last) = merged {
        union = add(union, last.end_ms - last.start_ms)?;
    }
    Ok(Some(UsageTimeTotals {
        observed_wall_clock_ms: union,
        observed_execution_ms: execution,
    }))
}

/// Coverage is independent of billed / API-equivalent / unknown cost columns.
/// A missing expected denominator never invents a ratio or treats unknown as 100%.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageCoverageObservation {
    pub observed_calls: usize,
    /// None means the expected set is unknown; do not invent coverage.
    pub expected_calls: Option<usize>,
    pub observed_time_ms: u64,
    /// None means the declared observation window is unknown.
    pub expected_time_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageCoverageReport {
    pub observed_calls: usize,
    pub expected_calls: Option<usize>,
    pub observed_time_ms: u64,
    pub expected_time_ms: Option<u64>,
    /// Integer ratio only when both sides are known; never a float percent.
    pub call_coverage: Option<(usize, usize)>,
    pub time_coverage: Option<(u64, u64)>,
    /// Always true: adapters must keep coverage out of cost columns.
    pub coverage_is_not_a_cost_column: bool,
}

pub fn usage_coverage_report(obs: &UsageCoverageObservation) -> Result<UsageCoverageReport> {
    if obs
        .expected_calls
        .is_some_and(|expected| obs.observed_calls > expected)
    {
        return Err(UsageError::Invalid("observed calls exceed expected"));
    }
    if obs
        .expected_time_ms
        .is_some_and(|expected| obs.observed_time_ms > expected)
    {
        return Err(UsageError::Invalid("observed time exceeds expected window"));
    }
    let call_coverage = obs
        .expected_calls
        .map(|expected| (obs.observed_calls, expected));
    let time_coverage = obs
        .expected_time_ms
        .map(|expected| (obs.observed_time_ms, expected));
    Ok(UsageCoverageReport {
        observed_calls: obs.observed_calls,
        expected_calls: obs.expected_calls,
        observed_time_ms: obs.observed_time_ms,
        expected_time_ms: obs.expected_time_ms,
        call_coverage,
        time_coverage,
        coverage_is_not_a_cost_column: true,
    })
}

/// Append-only correction. The original receipt body stays readable; totals that
/// apply corrections must cite the correction IDs for audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageCorrection {
    pub correction_id: String,
    pub project_id: String,
    pub target_receipt_id: String,
    pub corrected_at_ms: u64,
    pub reason: String,
    pub prior_cost: UsageCost,
    pub new_cost: UsageCost,
    pub actor: String,
}

impl UsageCorrection {
    fn validate(&self) -> Result<()> {
        if ![
            &self.correction_id,
            &self.project_id,
            &self.target_receipt_id,
            &self.reason,
            &self.actor,
        ]
        .into_iter()
        .all(|s| text(s))
        {
            return Err(UsageError::Invalid("correction identity"));
        }
        for cost in [&self.prior_cost, &self.new_cost] {
            if let Some(m) = cost.amount() {
                if m.currency.len() != 3 || !m.currency.bytes().all(|b| b.is_ascii_uppercase()) {
                    return Err(UsageError::Invalid("currency"));
                }
            }
            if let UsageCost::ApiEquivalentEstimate {
                pricing_version, ..
            } = cost
            {
                if !text(pricing_version) {
                    return Err(UsageError::Invalid("pricing version"));
                }
            }
        }
        if self.prior_cost == self.new_cost {
            return Err(UsageError::Invalid("noop correction"));
        }
        Ok(())
    }
}

/// Apply append-only corrections onto already-deduped receipts. Each target may
/// receive at most one correction in the supplied batch; unknown targets fail.
pub fn apply_usage_corrections(
    project: &str,
    receipts: &[UsageReceipt],
    corrections: &[UsageCorrection],
) -> Result<(Vec<UsageReceipt>, Vec<String>)> {
    if !text(project) {
        return Err(UsageError::Invalid("project"));
    }
    let unique = deduplicate_usage(project, receipts)?;
    let mut by_id: BTreeMap<&str, UsageReceipt> = unique
        .into_iter()
        .map(|r| (r.receipt_id.as_str(), r.clone()))
        .collect();
    let mut applied = Vec::new();
    let mut seen_targets = BTreeSet::new();
    let mut seen_ids = BTreeSet::new();
    for correction in corrections {
        correction.validate()?;
        if correction.project_id != project {
            return Err(UsageError::Invalid("cross-project correction"));
        }
        if !seen_ids.insert(correction.correction_id.as_str()) {
            return Err(UsageError::Conflict);
        }
        if !seen_targets.insert(correction.target_receipt_id.as_str()) {
            return Err(UsageError::CorrectionAudit);
        }
        let Some(receipt) = by_id.get_mut(correction.target_receipt_id.as_str()) else {
            return Err(UsageError::CorrectionAudit);
        };
        if receipt.cost != correction.prior_cost {
            return Err(UsageError::CorrectionAudit);
        }
        receipt.cost = correction.new_cost.clone();
        applied.push(correction.correction_id.clone());
    }
    Ok((by_id.into_values().collect(), applied))
}

/// Recorded allocation decision retained for audit (rule + conserved shares).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageAllocationRecord {
    pub allocation_id: String,
    pub project_id: String,
    pub receipt_id: String,
    pub rule: Option<String>,
    pub shares: Vec<UsageAllocation>,
    pub recorded_at_ms: u64,
}

impl UsageAllocationRecord {
    pub fn validate_against(&self, receipt: &UsageReceipt) -> Result<()> {
        if !text(&self.allocation_id) || self.project_id != receipt.project_id {
            return Err(UsageError::Invalid("allocation record"));
        }
        if self.receipt_id != receipt.receipt_id {
            return Err(UsageError::Invalid("allocation receipt mismatch"));
        }
        let _ = allocate_usage_cost(receipt, &self.shares, self.rule.as_deref())?;
        Ok(())
    }
}

/// Binding view: execution + task + occurrence-time mainline on a deduped receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageOccurrenceBinding {
    pub receipt_id: String,
    pub call_id: String,
    pub execution_id: String,
    pub work_id: String,
    /// Occurrence-time mainline; None when shared/unallocated at occurrence.
    pub occurrence_mainline_id: Option<Id>,
}

pub fn usage_occurrence_bindings(
    project: &str,
    receipts: &[UsageReceipt],
) -> Result<Vec<UsageOccurrenceBinding>> {
    let unique = deduplicate_usage(project, receipts)?;
    Ok(unique
        .into_iter()
        .map(|r| UsageOccurrenceBinding {
            receipt_id: r.receipt_id.clone(),
            call_id: r.call_id.clone(),
            execution_id: r.attribution.execution_id.clone(),
            work_id: r.attribution.work_id.clone(),
            occurrence_mainline_id: r.attribution.workstream_id,
        })
        .collect())
}

/// Historical observation package for WS-043. Never an ETA surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageTimeObservationHandoff {
    pub project_id: String,
    pub cost_totals: UsageCostTotals,
    pub time_totals: Option<UsageTimeTotals>,
    pub coverage: UsageCoverageReport,
    pub applied_correction_ids: Vec<String>,
    /// Always true: measured facts only.
    pub is_historical_observation: bool,
    /// Always true: cumulative duration fields are not estimated remaining time.
    pub is_not_estimated_remaining_time: bool,
    /// Parallel wall-clock and summed execution remain distinct when present.
    pub wall_clock_distinct_from_execution_sum: bool,
}

pub fn usage_observation_handoff(
    project: &str,
    receipts: &[UsageReceipt],
    corrections: &[UsageCorrection],
    intervals: Option<&[UsageTimeInterval]>,
    coverage: &UsageCoverageObservation,
) -> Result<UsageTimeObservationHandoff> {
    let (corrected, applied) = apply_usage_corrections(project, receipts, corrections)?;
    let cost_totals = usage_cost_totals(project, &corrected)?;
    // Re-total after correction requires re-wrapping as receipts with same call IDs.
    // usage_cost_totals dedups again; corrected vec may not share call uniqueness if
    // we only mutated cost — call identities are unchanged, so dedup is stable.
    let time_totals = usage_time_totals(intervals)?;
    let coverage = usage_coverage_report(coverage)?;
    Ok(UsageTimeObservationHandoff {
        project_id: project.into(),
        cost_totals,
        time_totals,
        coverage,
        applied_correction_ids: applied,
        is_historical_observation: true,
        is_not_estimated_remaining_time: true,
        wall_clock_distinct_from_execution_sum: true,
    })
}

/// Refuse presenting cumulative / summed duration as estimated remaining time.
pub fn refuse_eta_from_cumulative_duration(label: &str, cumulative_duration_ms: u64) -> Result<()> {
    let lowered = label.trim().to_ascii_lowercase();
    let claims_eta = ["eta", "remaining", "estimate", "预计", "剩余", "预估"]
        .iter()
        .any(|needle| lowered.contains(needle));
    if claims_eta {
        let _ = cumulative_duration_ms;
        return Err(UsageError::EtaFromCumulativeDuration);
    }
    Ok(())
}

/// Deduplicate time intervals by execution identity before wall/execution totals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageExecutionInterval {
    pub execution_id: String,
    pub interval: UsageTimeInterval,
}

pub fn deduplicate_execution_intervals(
    intervals: &[UsageExecutionInterval],
) -> Result<Vec<UsageTimeInterval>> {
    let mut by_exec = BTreeMap::new();
    for item in intervals {
        if !text(&item.execution_id) {
            return Err(UsageError::Invalid("execution interval identity"));
        }
        if item.interval.end_ms < item.interval.start_ms {
            return Err(UsageError::Invalid("reversed interval"));
        }
        if let Some(old) = by_exec.insert(item.execution_id.as_str(), item.interval) {
            if old != item.interval {
                return Err(UsageError::Conflict);
            }
        }
    }
    Ok(by_exec.into_values().collect())
}
