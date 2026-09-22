//! Bounded fact snapshot and source-quality labeling (DEC-011).
//!
//! Pure construction from fixed inputs: no Store, filesystem, network, model,
//! Git, AST, or implicit wall-clock. Callers supply `as_of` and any workspace
//! observations. Missing evidence stays missing — never zero-filled as verified.
use crate::{Error, Result, ensure_public_data};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const FACT_SNAPSHOT_SCHEMA_ID: &str = "awr-fact-snapshot-v1";
pub const FACT_SNAPSHOT_SCHEMA_VERSION: u32 = 1;
pub const FACT_COLLECTOR_VERSION: u32 = 1;

/// Hard ceilings for a single snapshot assembly. Callers may tighten, never raise.
pub const FACT_SNAPSHOT_MAX_CANDIDATES: usize = 256;
pub const FACT_SNAPSHOT_MAX_BYTES: usize = 256 * 1024;
pub const FACT_SNAPSHOT_MAX_SCAN_OPS: u64 = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalState {
    Known,
    Missing,
    Stale,
    Conflicting,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalBasis {
    SourceDeclared,
    RuntimeRecorded,
    HostAsserted,
    LocallyObserved,
    RuleDerived,
}

/// Fact = recorded with identity; observation = host/local, not independently
/// verified; inference = rule_derived and must never be written as verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalClassification {
    Fact,
    Observation,
    Inference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactSignal {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub state: SignalState,
    pub basis: SignalBasis,
    pub classification: SignalClassification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidation: Option<String>,
}

impl FactSignal {
    pub fn missing(field: &str, basis: SignalBasis) -> Self {
        Self {
            field: field.into(),
            value: None,
            unit: None,
            state: SignalState::Missing,
            basis,
            classification: classification_for(basis, SignalState::Missing),
            origin_ref: None,
            observed_at: None,
            scope: None,
            invalidation: None,
        }
    }

    pub fn known(
        field: &str,
        value: Value,
        basis: SignalBasis,
        origin_ref: Option<String>,
        observed_at: Option<i64>,
    ) -> Self {
        let classification = classification_for(basis, SignalState::Known);
        Self {
            field: field.into(),
            value: Some(value),
            unit: None,
            state: SignalState::Known,
            basis,
            classification,
            origin_ref,
            observed_at,
            scope: None,
            invalidation: None,
        }
    }

    pub fn is_verified_fact(&self) -> bool {
        self.state == SignalState::Known
            && self.classification == SignalClassification::Fact
            && matches!(
                self.basis,
                SignalBasis::SourceDeclared
                    | SignalBasis::RuntimeRecorded
                    | SignalBasis::LocallyObserved
            )
    }
}

fn classification_for(basis: SignalBasis, state: SignalState) -> SignalClassification {
    if state != SignalState::Known {
        return match basis {
            SignalBasis::RuleDerived => SignalClassification::Inference,
            SignalBasis::HostAsserted | SignalBasis::LocallyObserved => {
                SignalClassification::Observation
            }
            SignalBasis::SourceDeclared | SignalBasis::RuntimeRecorded => {
                SignalClassification::Fact
            }
        };
    }
    match basis {
        SignalBasis::RuleDerived => SignalClassification::Inference,
        SignalBasis::HostAsserted => SignalClassification::Observation,
        SignalBasis::LocallyObserved => SignalClassification::Observation,
        SignalBasis::SourceDeclared | SignalBasis::RuntimeRecorded => SignalClassification::Fact,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityVersionRef {
    pub entity_kind: String,
    pub entity_id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_contract_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotScope {
    pub read_scope: String,
    #[serde(default)]
    pub included: Vec<String>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotCoverage {
    #[serde(default)]
    pub required_fields: Vec<String>,
    #[serde(default)]
    pub observed_fields: Vec<String>,
    #[serde(default)]
    pub missing: Vec<String>,
    #[serde(default)]
    pub stale: Vec<String>,
    #[serde(default)]
    pub conflicting: Vec<String>,
    #[serde(default)]
    pub host_asserted_only: Vec<String>,
    #[serde(default)]
    pub unsupported: Vec<String>,
    pub coverage_note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotLimits {
    pub max_candidates: usize,
    pub max_bytes: usize,
    pub max_scan_ops: u64,
    pub candidates_seen: usize,
    pub bytes_used: usize,
    pub scan_ops: u64,
    pub omitted_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceFactSupply {
    /// Caller collected via the explicit WorkspaceFacts entry (not prepare).
    ExplicitCollection,
    /// Host supplied base/head/tree and coverage without AWR scanning.
    HostSupplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceDiffCoverage {
    /// A git-style diff was supplied and bound to base/head/tree.
    DiffBound,
    /// Partial paths or truncated scan; incomplete ≠ no change.
    Partial,
    /// No diff material; changed_lines and risk stay unset.
    Absent,
}

/// Optional workspace observations. Without a bound git diff, `changed_lines`
/// and low-risk labels must stay unset (never coerced to 0 / low-risk).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceFacts {
    pub supply: WorkspaceFactSupply,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collected_at: Option<i64>,
    pub coverage: WorkspaceDiffCoverage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_lines: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_risk: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl WorkspaceFacts {
    /// Host or explicit collection without a git diff: coverage absent, no zero-fill.
    pub fn without_diff(supply: WorkspaceFactSupply, collected_at: Option<i64>) -> Self {
        Self {
            supply,
            base: None,
            head: None,
            tree: None,
            collected_at,
            coverage: WorkspaceDiffCoverage::Absent,
            changed_lines: None,
            change_risk: None,
            notes: vec!["git_diff_absent".into()],
        }
    }

    pub fn with_bound_diff(
        supply: WorkspaceFactSupply,
        base: String,
        head: String,
        tree: Option<String>,
        collected_at: Option<i64>,
        changed_lines: u64,
        change_risk: Option<String>,
    ) -> Result<Self> {
        if base.trim().is_empty() || head.trim().is_empty() {
            return Err(Error::InvalidInput(
                "workspace facts with a bound diff require nonempty base and head".into(),
            ));
        }
        Ok(Self {
            supply,
            base: Some(base),
            head: Some(head),
            tree,
            collected_at,
            coverage: WorkspaceDiffCoverage::DiffBound,
            changed_lines: Some(changed_lines),
            change_risk,
            notes: vec![],
        })
    }

    pub fn validate(&self) -> Result<()> {
        ensure_public_data(self)?;
        match self.coverage {
            WorkspaceDiffCoverage::Absent => {
                if self.changed_lines.is_some() {
                    return Err(Error::InvalidInput(
                        "absent git diff must not emit changed_lines (including 0)".into(),
                    ));
                }
                if self
                    .change_risk
                    .as_deref()
                    .is_some_and(|r| r == "low" || r == "low_risk")
                {
                    return Err(Error::InvalidInput(
                        "absent git diff must not emit low-risk change labels".into(),
                    ));
                }
            }
            WorkspaceDiffCoverage::Partial => {
                if self.changed_lines == Some(0) {
                    return Err(Error::InvalidInput(
                        "partial workspace coverage must not coerce incomplete scans to changed_lines=0"
                            .into(),
                    ));
                }
                if self
                    .change_risk
                    .as_deref()
                    .is_some_and(|r| r == "low" || r == "low_risk")
                {
                    return Err(Error::InvalidInput(
                        "partial workspace coverage must not emit low-risk".into(),
                    ));
                }
            }
            WorkspaceDiffCoverage::DiffBound => {
                if self.base.as_ref().is_none_or(|s| s.trim().is_empty())
                    || self.head.as_ref().is_none_or(|s| s.trim().is_empty())
                {
                    return Err(Error::InvalidInput(
                        "diff-bound workspace facts require base and head".into(),
                    ));
                }
                if self.changed_lines.is_none() {
                    return Err(Error::InvalidInput(
                        "diff-bound workspace facts require an observed changed_lines count".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactSnapshot {
    pub schema_id: String,
    pub schema_version: u32,
    pub collector_version: u32,
    pub identity: SnapshotIdentity,
    pub as_of: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_hash: Option<String>,
    #[serde(default)]
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub runtime_observation_refs: Vec<String>,
    pub scope: SnapshotScope,
    pub coverage: SnapshotCoverage,
    pub limits: SnapshotLimits,
    #[serde(default)]
    pub entity_versions: Vec<EntityVersionRef>,
    #[serde(default)]
    pub signals: Vec<FactSignal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_facts: Option<WorkspaceFacts>,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactSnapshotLimitsInput {
    pub max_candidates: usize,
    pub max_bytes: usize,
    pub max_scan_ops: u64,
}

impl Default for FactSnapshotLimitsInput {
    fn default() -> Self {
        Self {
            max_candidates: FACT_SNAPSHOT_MAX_CANDIDATES,
            max_bytes: FACT_SNAPSHOT_MAX_BYTES,
            max_scan_ops: FACT_SNAPSHOT_MAX_SCAN_OPS,
        }
    }
}

impl FactSnapshotLimitsInput {
    pub fn capped(self) -> Result<Self> {
        if self.max_candidates == 0
            || self.max_bytes == 0
            || self.max_scan_ops == 0
            || self.max_candidates > FACT_SNAPSHOT_MAX_CANDIDATES
            || self.max_bytes > FACT_SNAPSHOT_MAX_BYTES
            || self.max_scan_ops > FACT_SNAPSHOT_MAX_SCAN_OPS
        {
            return Err(Error::InvalidInput(format!(
                "fact snapshot limits must be within 1..={}/{}/{}",
                FACT_SNAPSHOT_MAX_CANDIDATES, FACT_SNAPSHOT_MAX_BYTES, FACT_SNAPSHOT_MAX_SCAN_OPS
            )));
        }
        Ok(self)
    }
}

/// Fixed inputs for pure snapshot construction. No I/O.
#[derive(Debug, Clone)]
pub struct FactSnapshotInput {
    pub identity: SnapshotIdentity,
    pub as_of: i64,
    pub policy_id: Option<String>,
    pub policy_version: Option<u32>,
    pub policy_hash: Option<String>,
    pub source_refs: Vec<String>,
    pub runtime_observation_refs: Vec<String>,
    pub entity_versions: Vec<EntityVersionRef>,
    pub signals: Vec<FactSignal>,
    pub required_fields: Vec<String>,
    pub workspace_facts: Option<WorkspaceFacts>,
    pub read_scope: String,
    pub included: Vec<String>,
    pub limits: FactSnapshotLimitsInput,
    /// Candidate rows considered before truncation (e.g. diagnostic entries).
    pub candidates_seen: usize,
    /// Logical scan operations already performed by the caller (not by this builder).
    pub scan_ops: u64,
}

/// Reject same-entity conflicting versions inside one snapshot.
pub fn reject_entity_version_conflicts(versions: &[EntityVersionRef]) -> Result<()> {
    let mut seen: BTreeMap<(String, String), String> = BTreeMap::new();
    for v in versions {
        if v.entity_kind.trim().is_empty()
            || v.entity_id.trim().is_empty()
            || v.version.trim().is_empty()
        {
            return Err(Error::InvalidInput(
                "entity version refs require nonempty kind, id, and version".into(),
            ));
        }
        let key = (v.entity_kind.clone(), v.entity_id.clone());
        if let Some(prior) = seen.get(&key) {
            if prior != &v.version {
                return Err(Error::SourceConflict(format!(
                    "entity version conflict in one fact snapshot: {}/{} has both {} and {}",
                    v.entity_kind, v.entity_id, prior, v.version
                )));
            }
        } else {
            seen.insert(key, v.version.clone());
        }
    }
    Ok(())
}

fn validate_signal(signal: &FactSignal) -> Result<()> {
    ensure_public_data(signal)?;
    if signal.field.trim().is_empty() || signal.field.len() > 4096 {
        return Err(Error::InvalidInput(
            "fact signal field must be a nonempty bounded name".into(),
        ));
    }
    if signal.state == SignalState::Known && signal.value.is_none() {
        return Err(Error::InvalidInput(format!(
            "known signal {} requires a value",
            signal.field
        )));
    }
    if matches!(
        signal.state,
        SignalState::Missing | SignalState::Unsupported | SignalState::Unknown
    ) && signal.value.is_some()
    {
        return Err(Error::InvalidInput(format!(
            "signal {} with state {:?} must not carry a value",
            signal.field, signal.state
        )));
    }
    // Inference must never be labeled as independently verified fact.
    if signal.basis == SignalBasis::RuleDerived
        && signal.classification == SignalClassification::Fact
    {
        return Err(Error::InvalidInput(format!(
            "rule_derived signal {} must not be classified as verified fact",
            signal.field
        )));
    }
    if signal.basis == SignalBasis::HostAsserted
        && signal.classification == SignalClassification::Fact
        && signal.state == SignalState::Known
    {
        return Err(Error::InvalidInput(format!(
            "host_asserted signal {} is an observation, not a verified fact",
            signal.field
        )));
    }
    Ok(())
}

fn build_coverage(required: &[String], signals: &[FactSignal]) -> SnapshotCoverage {
    let mut observed = BTreeSet::new();
    let mut missing = BTreeSet::new();
    let mut stale = BTreeSet::new();
    let mut conflicting = BTreeSet::new();
    let mut host_asserted_only = BTreeSet::new();
    let mut unsupported = BTreeSet::new();
    for s in signals {
        match s.state {
            SignalState::Known => {
                observed.insert(s.field.clone());
                if s.basis == SignalBasis::HostAsserted {
                    host_asserted_only.insert(s.field.clone());
                }
            }
            SignalState::Missing | SignalState::Unknown => {
                missing.insert(s.field.clone());
            }
            SignalState::Stale => {
                stale.insert(s.field.clone());
                observed.insert(s.field.clone());
            }
            SignalState::Conflicting => {
                conflicting.insert(s.field.clone());
                observed.insert(s.field.clone());
            }
            SignalState::Unsupported => {
                unsupported.insert(s.field.clone());
            }
        }
    }
    for field in required {
        if !signals.iter().any(|s| &s.field == field) {
            missing.insert(field.clone());
        }
    }
    SnapshotCoverage {
        required_fields: required.to_vec(),
        observed_fields: observed.into_iter().collect(),
        missing: missing.into_iter().collect(),
        stale: stale.into_iter().collect(),
        conflicting: conflicting.into_iter().collect(),
        host_asserted_only: host_asserted_only.into_iter().collect(),
        unsupported: unsupported.into_iter().collect(),
        coverage_note: "coverage is a field count ratio, not probability".into(),
    }
}

fn content_hash_bytes(snapshot: &FactSnapshot) -> Result<String> {
    // Hash semantic payload without the content_hash field itself.
    let payload = json!({
        "schema_id": snapshot.schema_id,
        "schema_version": snapshot.schema_version,
        "collector_version": snapshot.collector_version,
        "identity": snapshot.identity,
        "as_of": snapshot.as_of,
        "policy_id": snapshot.policy_id,
        "policy_version": snapshot.policy_version,
        "policy_hash": snapshot.policy_hash,
        "source_refs": snapshot.source_refs,
        "runtime_observation_refs": snapshot.runtime_observation_refs,
        "scope": snapshot.scope,
        "coverage": snapshot.coverage,
        "limits": {
            "max_candidates": snapshot.limits.max_candidates,
            "max_bytes": snapshot.limits.max_bytes,
            "max_scan_ops": snapshot.limits.max_scan_ops,
            "candidates_seen": snapshot.limits.candidates_seen,
            "scan_ops": snapshot.limits.scan_ops,
            "omitted_count": snapshot.limits.omitted_count,
        },
        "entity_versions": snapshot.entity_versions,
        "signals": snapshot.signals,
        "workspace_facts": snapshot.workspace_facts,
    });
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&payload)?)
    ))
}

/// Build a bounded FactSnapshot from fixed inputs. Deterministic for identical inputs.
pub fn build_fact_snapshot(input: FactSnapshotInput) -> Result<FactSnapshot> {
    ensure_public_data(&input.identity)?;
    if input.as_of < 0 {
        return Err(Error::InvalidInput(
            "fact snapshot as_of must be a non-negative timestamp".into(),
        ));
    }
    if input.read_scope.trim().is_empty() {
        return Err(Error::InvalidInput(
            "fact snapshot requires a nonempty read_scope".into(),
        ));
    }
    let limits = input.limits.capped()?;
    reject_entity_version_conflicts(&input.entity_versions)?;
    if let Some(ws) = &input.workspace_facts {
        ws.validate()?;
    }
    for signal in &input.signals {
        validate_signal(signal)?;
    }

    let mut truncated = false;
    let mut truncation_reason = None;
    let mut omitted_count = 0usize;
    let mut signals = input.signals;
    let candidates_seen = input.candidates_seen.max(signals.len());

    if candidates_seen > limits.max_candidates {
        truncated = true;
        truncation_reason = Some("candidates_exceeded".into());
        if signals.len() > limits.max_candidates {
            omitted_count += signals.len() - limits.max_candidates;
            signals.truncate(limits.max_candidates);
        }
    }
    if input.scan_ops > limits.max_scan_ops {
        truncated = true;
        truncation_reason = Some(truncation_reason.unwrap_or_else(|| "scan_ops_exceeded".into()));
    }

    // Bound serialized signal payload bytes without inventing values.
    let mut kept = Vec::with_capacity(signals.len());
    let mut bytes_used = 0usize;
    for signal in signals {
        let encoded = serde_json::to_vec(&signal)?;
        if bytes_used + encoded.len() > limits.max_bytes {
            truncated = true;
            truncation_reason = Some("return_bytes_exceeded".into());
            omitted_count += 1;
            continue;
        }
        bytes_used += encoded.len();
        kept.push(signal);
    }

    let coverage = build_coverage(&input.required_fields, &kept);
    let mut snapshot = FactSnapshot {
        schema_id: FACT_SNAPSHOT_SCHEMA_ID.into(),
        schema_version: FACT_SNAPSHOT_SCHEMA_VERSION,
        collector_version: FACT_COLLECTOR_VERSION,
        identity: input.identity,
        as_of: input.as_of,
        policy_id: input.policy_id,
        policy_version: input.policy_version,
        policy_hash: input.policy_hash,
        source_refs: input.source_refs,
        runtime_observation_refs: input.runtime_observation_refs,
        scope: SnapshotScope {
            read_scope: input.read_scope,
            included: input.included,
            truncated,
            truncation_reason,
        },
        coverage,
        limits: SnapshotLimits {
            max_candidates: limits.max_candidates,
            max_bytes: limits.max_bytes,
            max_scan_ops: limits.max_scan_ops,
            candidates_seen,
            bytes_used,
            scan_ops: input.scan_ops,
            omitted_count,
        },
        entity_versions: input.entity_versions,
        signals: kept,
        workspace_facts: input.workspace_facts,
        content_hash: String::new(),
    };
    snapshot.content_hash = content_hash_bytes(&snapshot)?;
    ensure_public_data(&snapshot)?;
    Ok(snapshot)
}

/// Signal for a required field when git diff was not supplied.
pub fn missing_git_diff_signal() -> FactSignal {
    FactSignal {
        field: "workspace.changed_lines".into(),
        value: None,
        unit: Some("lines".into()),
        state: SignalState::Missing,
        basis: SignalBasis::LocallyObserved,
        classification: SignalClassification::Observation,
        origin_ref: None,
        observed_at: None,
        scope: Some("workspace".into()),
        invalidation: Some("requires_explicit_git_diff".into()),
    }
}

/// Change-risk must stay unsupported without a bound diff — never low-risk.
pub fn unsupported_change_risk_signal() -> FactSignal {
    FactSignal {
        field: "workspace.change_risk".into(),
        value: None,
        unit: None,
        state: SignalState::Unsupported,
        basis: SignalBasis::RuleDerived,
        classification: SignalClassification::Inference,
        origin_ref: None,
        observed_at: None,
        scope: Some("workspace".into()),
        invalidation: Some("risk_requires_bound_diff".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_diff_rejects_zero_changed_lines() {
        let mut ws = WorkspaceFacts::without_diff(WorkspaceFactSupply::HostSupplied, Some(1));
        ws.changed_lines = Some(0);
        assert!(ws.validate().is_err());
    }
}
