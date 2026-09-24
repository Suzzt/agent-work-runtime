//! Map one existing prepare/assess read view into a bounded FactSnapshot (DEC-011).
//!
//! Reuses caller-supplied readiness / management / context results — does not open a
//! second Store query stack, recurse the repo, or run Git / AST / network commands.
use awr_core::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Fixed ceilings for the runtime mapping path (may only tighten core ceilings).
pub const PREPARE_FACT_MAX_CANDIDATES: usize = FACT_SNAPSHOT_MAX_CANDIDATES;
pub const PREPARE_FACT_MAX_BYTES: usize = FACT_SNAPSHOT_MAX_BYTES;
pub const PREPARE_FACT_MAX_SCAN_OPS: u64 = FACT_SNAPSHOT_MAX_SCAN_OPS;

/// One already-fetched read-only query result used to assemble facts.
/// Prefer the prepare envelope or a single assess receipt — not a fresh re-query.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedFactView {
    pub project_id: Option<String>,
    pub work_key: String,
    pub work_id: Option<String>,
    pub branch_id: Option<String>,
    pub work_revision: Option<String>,
    pub source_revision: Option<String>,
    pub work_contract_hash: Option<String>,
    pub as_of: i64,
    /// Ready flag from the same `work_readiness` call already opened by prepare/assess.
    pub ready: Option<bool>,
    #[serde(default)]
    pub diagnostics: Vec<Value>,
    #[serde(default)]
    pub active_claim_ids: Vec<String>,
    /// Management assess object from the same prepare/assess snapshot (optional).
    #[serde(default)]
    pub management: Option<Value>,
    /// Context completeness fragment when prepare already compiled context.
    #[serde(default)]
    pub context_complete: Option<bool>,
    #[serde(default)]
    pub context_issues: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<String>,
    #[serde(default)]
    pub runtime_observation_refs: Vec<String>,
    /// Host-asserted management observation fields (labeled, never verified facts).
    #[serde(default)]
    pub host_observation: Option<ManagementObservation>,
    /// Optional workspace facts: host-supplied or explicit collection only.
    #[serde(default)]
    pub workspace_facts: Option<WorkspaceFacts>,
    #[serde(default)]
    pub limits: Option<FactSnapshotLimitsInput>,
}

/// Build a FactSnapshot from one prepared view. Scan ops count = 1 (the reused query).
pub fn fact_snapshot_from_prepared_view(view: &PreparedFactView) -> Result<FactSnapshot> {
    ensure_public_data(view)?;
    if view.work_key.trim().is_empty() {
        return Err(Error::InvalidInput(
            "prepared fact view requires a work_key".into(),
        ));
    }
    let limits = view.limits.clone().unwrap_or_default().capped()?;

    let mut entity_versions = Vec::new();
    if let (Some(work_id), Some(rev)) = (&view.work_id, &view.work_revision) {
        entity_versions.push(EntityVersionRef {
            entity_kind: "work".into(),
            entity_id: work_id.clone(),
            version: rev.clone(),
        });
    }
    if let Some(src) = &view.source_revision {
        let entity_id = view
            .project_id
            .clone()
            .unwrap_or_else(|| view.work_key.clone());
        entity_versions.push(EntityVersionRef {
            entity_kind: "source".into(),
            entity_id,
            version: src.clone(),
        });
    }
    // Management contract fingerprint as a versioned entity when present.
    if let Some(fp) = view
        .management
        .as_ref()
        .and_then(|m| m.get("contract_fingerprint"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        entity_versions.push(EntityVersionRef {
            entity_kind: "work_contract".into(),
            entity_id: view.work_key.clone(),
            version: fp.into(),
        });
    }

    let mut signals = Vec::new();
    let mut required = vec![
        "work.ready".into(),
        "work.contract_hash".into(),
        "workspace.changed_lines".into(),
        "workspace.change_risk".into(),
    ];

    match view.ready {
        Some(ready) => signals.push(FactSignal::known(
            "work.ready",
            Value::Bool(ready),
            SignalBasis::RuntimeRecorded,
            Some("work_readiness".into()),
            Some(view.as_of),
        )),
        None => signals.push(FactSignal::missing(
            "work.ready",
            SignalBasis::RuntimeRecorded,
        )),
    }

    match &view.work_contract_hash {
        Some(hash) if !hash.is_empty() => signals.push(FactSignal::known(
            "work.contract_hash",
            Value::String(hash.clone()),
            SignalBasis::SourceDeclared,
            Some("contract_fingerprint".into()),
            Some(view.as_of),
        )),
        _ => {
            // Prefer management contract_fingerprint when the view omitted the field.
            if let Some(fp) = view
                .management
                .as_ref()
                .and_then(|m| m.get("contract_fingerprint"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                signals.push(FactSignal::known(
                    "work.contract_hash",
                    Value::String(fp.into()),
                    SignalBasis::SourceDeclared,
                    Some("management.contract_fingerprint".into()),
                    Some(view.as_of),
                ));
            } else {
                signals.push(FactSignal::missing(
                    "work.contract_hash",
                    SignalBasis::SourceDeclared,
                ));
            }
        }
    }

    for (i, diag) in view.diagnostics.iter().enumerate() {
        let code = diag
            .get("code")
            .or_else(|| diag.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("diagnostic");
        signals.push(FactSignal::known(
            &format!("readiness.diagnostic.{i}"),
            Value::String(code.into()),
            SignalBasis::RuntimeRecorded,
            Some("work_readiness.diagnostics".into()),
            Some(view.as_of),
        ));
    }

    if let Some(complete) = view.context_complete {
        signals.push(FactSignal::known(
            "context.complete",
            Value::Bool(complete),
            SignalBasis::RuntimeRecorded,
            Some("context.completeness".into()),
            Some(view.as_of),
        ));
        required.push("context.complete".into());
    }

    for issue in &view.context_issues {
        signals.push(FactSignal {
            field: format!("context.issue.{issue}"),
            value: Some(Value::String(issue.clone())),
            unit: None,
            state: SignalState::Known,
            basis: SignalBasis::RuntimeRecorded,
            classification: SignalClassification::Fact,
            origin_ref: Some("context.completeness.issues".into()),
            observed_at: Some(view.as_of),
            scope: Some("context".into()),
            invalidation: None,
        });
    }

    if let Some(obs) = &view.host_observation {
        for (field, value) in [
            (
                "management.single_outcome",
                obs.single_outcome.map(Value::Bool),
            ),
            (
                "management.bounded_scope",
                obs.bounded_scope.map(Value::Bool),
            ),
            (
                "management.single_executor",
                obs.single_executor.map(Value::Bool),
            ),
            (
                "management.no_deferred_wait",
                obs.no_deferred_wait.map(Value::Bool),
            ),
            ("management.plan_valid", obs.plan_valid.map(Value::Bool)),
            (
                "management.outcome_known",
                obs.outcome_known.map(Value::Bool),
            ),
        ] {
            match value {
                Some(v) => signals.push(FactSignal {
                    field: field.into(),
                    value: Some(v),
                    unit: None,
                    state: SignalState::Known,
                    basis: SignalBasis::HostAsserted,
                    classification: SignalClassification::Observation,
                    origin_ref: Some("host_observation".into()),
                    observed_at: Some(obs.observed_at),
                    scope: Some("management".into()),
                    invalidation: Some("host_assertion_not_independently_verified".into()),
                }),
                None => signals.push(FactSignal::missing(field, SignalBasis::HostAsserted)),
            }
        }
    }

    // Stale management receipt: prior assess with mismatched contract fingerprint.
    if let Some(mgmt) = &view.management {
        if mgmt.get("stale") == Some(&Value::Bool(true)) {
            signals.push(FactSignal {
                field: "management.assessment".into(),
                value: mgmt.get("contract_fingerprint").cloned(),
                unit: None,
                state: SignalState::Stale,
                basis: SignalBasis::RuntimeRecorded,
                classification: SignalClassification::Fact,
                origin_ref: Some("management.assessed".into()),
                observed_at: Some(view.as_of),
                scope: Some("management".into()),
                invalidation: Some("work_contract_changed".into()),
            });
        }
        if let Some(mode) = mgmt
            .pointer("/decision/mode")
            .or_else(|| mgmt.pointer("/assessment/decision/mode"))
            .and_then(|v| v.as_str())
        {
            signals.push(FactSignal::known(
                "management.mode",
                Value::String(mode.into()),
                SignalBasis::RuleDerived,
                Some("decide_management".into()),
                Some(view.as_of),
            ));
            // Fix classification: rule_derived must be inference.
            if let Some(last) = signals.last_mut() {
                last.classification = SignalClassification::Inference;
            }
        }
    }

    let workspace_facts = view.workspace_facts.clone();
    match &workspace_facts {
        Some(ws) if ws.coverage == WorkspaceDiffCoverage::DiffBound => {
            if let Some(lines) = ws.changed_lines {
                signals.push(FactSignal {
                    field: "workspace.changed_lines".into(),
                    value: Some(Value::from(lines)),
                    unit: Some("lines".into()),
                    state: SignalState::Known,
                    basis: SignalBasis::LocallyObserved,
                    classification: SignalClassification::Observation,
                    origin_ref: Some("workspace_facts".into()),
                    observed_at: ws.collected_at,
                    scope: Some("workspace".into()),
                    invalidation: None,
                });
            }
            if let Some(risk) = &ws.change_risk {
                signals.push(FactSignal {
                    field: "workspace.change_risk".into(),
                    value: Some(Value::String(risk.clone())),
                    unit: None,
                    state: SignalState::Known,
                    basis: SignalBasis::RuleDerived,
                    classification: SignalClassification::Inference,
                    origin_ref: Some("workspace_facts".into()),
                    observed_at: ws.collected_at,
                    scope: Some("workspace".into()),
                    invalidation: None,
                });
            } else {
                signals.push(unsupported_change_risk_signal());
            }
        }
        Some(_) | None => {
            // Missing git diff: emit distinct missing/unsupported — never 0 / low-risk.
            signals.push(missing_git_diff_signal());
            signals.push(unsupported_change_risk_signal());
        }
    }

    let candidates_seen = view.diagnostics.len()
        + view.context_issues.len()
        + view.active_claim_ids.len()
        + signals.len();

    let identity = SnapshotIdentity {
        project_id: view.project_id.clone(),
        work_key: Some(view.work_key.clone()),
        work_id: view.work_id.clone(),
        branch_id: view.branch_id.clone(),
        work_contract_hash: view.work_contract_hash.clone().or_else(|| {
            view.management
                .as_ref()
                .and_then(|m| m.get("contract_fingerprint"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }),
    };

    let mut included = vec!["work_readiness".into()];
    if view.management.is_some() {
        included.push("management".into());
    }
    if view.context_complete.is_some() || !view.context_issues.is_empty() {
        included.push("context".into());
    }
    if workspace_facts.is_some() {
        included.push("workspace_facts".into());
    }

    build_fact_snapshot(FactSnapshotInput {
        identity,
        as_of: view.as_of,
        policy_id: Some("tip_management_v1".into()),
        policy_version: Some(1),
        policy_hash: None,
        source_refs: view.source_refs.clone(),
        runtime_observation_refs: view.runtime_observation_refs.clone(),
        entity_versions,
        signals,
        required_fields: required,
        workspace_facts,
        read_scope: "assessment_read_only".into(),
        included,
        limits,
        candidates_seen,
        // Exactly one reused read-only query result; prepare must not add scans.
        scan_ops: 1,
    })
}

/// Extract a PreparedFactView from an existing prepare JSON value (no Store I/O).
pub fn prepared_view_from_prepare_json(
    prepare: &Value,
    project_id: Option<String>,
    as_of: i64,
    workspace_facts: Option<WorkspaceFacts>,
) -> Result<PreparedFactView> {
    ensure_public_data(prepare)?;
    let work = prepare.get("work").unwrap_or(prepare);
    let work_key = work
        .get("external_key")
        .or_else(|| prepare.get("work_key"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::InvalidInput("prepare JSON missing work external_key".into()))?
        .to_string();
    let management = prepare.get("management").cloned();
    let contract = management
        .as_ref()
        .and_then(|m| m.get("contract_fingerprint"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let context = prepare.get("context");
    let completeness = context.and_then(|c| c.get("completeness"));
    let diagnostics = prepare
        .get("diagnostics")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let active_claims = prepare
        .get("active_claims")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    c.get("id")
                        .or_else(|| c.get("claim_id"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    let issues = completeness
        .and_then(|c| c.get("issues"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|i| {
                    i.as_str()
                        .map(str::to_string)
                        .or_else(|| i.get("code").and_then(|c| c.as_str()).map(str::to_string))
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(PreparedFactView {
        project_id,
        work_key,
        work_id: work.get("id").and_then(|v| v.as_str()).map(str::to_string),
        branch_id: completeness
            .and_then(|c| c.get("branch_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                prepare
                    .get("branch_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            }),
        work_revision: work.get("revision").map(|v| match v {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => v.to_string(),
        }),
        source_revision: work
            .pointer("/source_ref/source_revision")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        work_contract_hash: contract,
        as_of,
        ready: prepare.get("ready").and_then(|v| v.as_bool()),
        diagnostics,
        active_claim_ids: active_claims,
        management: management.clone(),
        context_complete: completeness
            .and_then(|c| c.get("complete"))
            .and_then(|v| v.as_bool()),
        context_issues: issues,
        source_refs: vec![],
        runtime_observation_refs: vec![],
        host_observation: management
            .as_ref()
            .and_then(|m| m.get("observation"))
            .and_then(|o| serde_json::from_value(o.clone()).ok()),
        workspace_facts,
        limits: None,
    })
}
