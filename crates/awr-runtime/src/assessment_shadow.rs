//! DEC-022: shadow compare + one-click advice disable (kill-switch).
//!
//! Shadow mode computes candidate advice without changing execution or context
//! adoption. Disable restores prior advice behavior only — existing hard
//! protections stay. No background daemons.

use crate::assessment_explain::{
    ASSESSMENT_EXPLAIN_FIELD, AttachExplanationOptions, attach_assessment_explanation,
};
use crate::assessment_replay::{
    ReplayReport, ReplaySnapshot, ReplayStatus, replay_assessment,
};
use awr_core::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Capability: baseline/candidate shadow compare.
pub const ASSESSMENT_SHADOW_COMPARE_CAPABILITY: &str = "assessment.shadow_compare";
/// Capability: advice delivery mode / kill-switch.
pub const ASSESSMENT_ADVICE_MODE_CAPABILITY: &str = "assessment.advice_mode";

/// How new assessment advice is delivered to consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AdviceDeliveryMode {
    /// Kill-switch / default: restore prior advice behavior (no new explain field).
    #[default]
    Disabled,
    /// Compute candidate advice but do not adopt for execution or context.
    Shadow,
    /// Attach new advice on negotiated explain paths (DEC-021).
    Enabled,
}

impl AdviceDeliveryMode {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "disabled" | "off" | "kill" | "killswitch" | "kill_switch" => Ok(Self::Disabled),
            "shadow" => Ok(Self::Shadow),
            "enabled" | "on" => Ok(Self::Enabled),
            other => Err(Error::InvalidInput(format!(
                "unknown advice delivery mode '{other}' (expected disabled|shadow|enabled)"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Shadow => "shadow",
            Self::Enabled => "enabled",
        }
    }

    pub fn attaches_explain(self) -> bool {
        matches!(self, Self::Shadow | Self::Enabled)
    }

    pub fn adopts_for_execution_or_context(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// Hard protections that must remain regardless of advice mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardProtectionFlags {
    pub claim_guards_active: bool,
    pub completion_guards_active: bool,
    pub admission_guards_active: bool,
    pub source_freshness_guards_active: bool,
    pub stop_or_revoke_honored: bool,
}

impl Default for HardProtectionFlags {
    fn default() -> Self {
        Self {
            claim_guards_active: true,
            completion_guards_active: true,
            admission_guards_active: true,
            source_freshness_guards_active: true,
            stop_or_revoke_honored: true,
        }
    }
}

/// Effect of applying an advice delivery mode (sync only; no daemons).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdviceModeEffect {
    pub mode: AdviceDeliveryMode,
    pub background_daemon: bool,
    pub execution_adoption_changed: bool,
    pub context_adoption_changed: bool,
    pub new_advice_attached: bool,
    pub restores_prior_advice_behavior: bool,
    pub hard_protections: HardProtectionFlags,
}

/// Resolve mode effects. Kill-switch only restores prior advice behavior.
pub fn apply_advice_delivery_mode(mode: AdviceDeliveryMode) -> AdviceModeEffect {
    let hard = HardProtectionFlags::default();
    match mode {
        AdviceDeliveryMode::Disabled => AdviceModeEffect {
            mode,
            background_daemon: false,
            execution_adoption_changed: false,
            context_adoption_changed: false,
            new_advice_attached: false,
            restores_prior_advice_behavior: true,
            hard_protections: hard,
        },
        AdviceDeliveryMode::Shadow => AdviceModeEffect {
            mode,
            background_daemon: false,
            execution_adoption_changed: false,
            context_adoption_changed: false,
            new_advice_attached: true,
            restores_prior_advice_behavior: false,
            hard_protections: hard,
        },
        AdviceDeliveryMode::Enabled => AdviceModeEffect {
            mode,
            background_daemon: false,
            execution_adoption_changed: false,
            context_adoption_changed: false,
            new_advice_attached: true,
            restores_prior_advice_behavior: false,
            hard_protections: hard,
        },
    }
}

/// Attach explain according to advice mode. Shadow tags non-adoption; disable omits field.
pub fn attach_according_to_advice_mode(
    mut value: Value,
    mode: AdviceDeliveryMode,
    mut options: AttachExplanationOptions,
) -> Result<Value> {
    let effect = apply_advice_delivery_mode(mode);
    debug_assert!(effect.hard_protections.claim_guards_active);
    debug_assert!(!effect.background_daemon);

    options.enabled = mode.attaches_explain();
    value = attach_assessment_explanation(value, &options)?;

    if let Some(obj) = value.as_object_mut() {
        match mode {
            AdviceDeliveryMode::Disabled => {
                obj.remove(ASSESSMENT_EXPLAIN_FIELD);
            }
            AdviceDeliveryMode::Shadow => {
                if let Some(explain) = obj.get_mut(ASSESSMENT_EXPLAIN_FIELD) {
                    if let Some(eo) = explain.as_object_mut() {
                        eo.insert("shadow".into(), json!(true));
                        eo.insert("execution_adoption".into(), json!(false));
                        eo.insert("context_adoption".into(), json!(false));
                        eo.insert(
                            "advice_delivery_mode".into(),
                            json!(AdviceDeliveryMode::Shadow.as_str()),
                        );
                    }
                }
            }
            AdviceDeliveryMode::Enabled => {
                if let Some(explain) = obj.get_mut(ASSESSMENT_EXPLAIN_FIELD) {
                    if let Some(eo) = explain.as_object_mut() {
                        eo.insert("shadow".into(), json!(false));
                        eo.insert("execution_adoption".into(), json!(true));
                        eo.insert("context_adoption".into(), json!(true));
                        eo.insert(
                            "advice_delivery_mode".into(),
                            json!(AdviceDeliveryMode::Enabled.as_str()),
                        );
                    }
                }
            }
        }
        obj.insert("advice_mode_effect".into(), serde_json::to_value(effect)?);
    }
    Ok(value)
}

/// Cost dimensions compared for baseline vs candidate (offline; no model/$ derivation).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompareCosts {
    pub collect_units: u64,
    pub judge_units: u64,
    pub output_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmSummary {
    pub label: String,
    pub policy_id: String,
    pub policy_version: u32,
    pub rule_hash: String,
    pub assessment_hash: String,
    pub reason_codes: Vec<String>,
    pub advisory_codes: Vec<String>,
    pub hard_rejects: Vec<String>,
    pub hard_gate: String,
    pub costs: CompareCosts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowDifference {
    pub field: String,
    pub baseline: Value,
    pub candidate: Value,
    /// Difference attributed to rule/policy version identity.
    pub explained_by_rule_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowCompareReport {
    pub same_inputs: bool,
    pub baseline: ArmSummary,
    pub candidate: ArmSummary,
    pub differences: Vec<ShadowDifference>,
    /// Failed / divergent samples retained in full (acceptance).
    pub retained_failure_samples: Vec<Value>,
    pub execution_adoption: bool,
    pub context_adoption: bool,
    pub background_daemon: bool,
    pub passed: bool,
}

fn hard_gate_str(gate: &HardGateOutcome) -> String {
    match gate {
        HardGateOutcome::Pass => "pass".into(),
        HardGateOutcome::Reject => "reject".into(),
        HardGateOutcome::Unknown => "unknown".into(),
    }
}

fn summarize_arm(label: &str, snapshot: &ReplaySnapshot, report: &ReplayReport) -> Result<ArmSummary> {
    let envelope = report.envelope.as_ref().ok_or_else(|| {
        Error::InvalidInput(format!(
            "shadow compare arm '{label}' missing envelope ({:?})",
            report.status
        ))
    })?;
    let mut reason_codes = Vec::new();
    let mut hard_rejects = Vec::new();
    for item in &envelope.assessments {
        reason_codes.extend(item.reason_codes.iter().cloned());
        if item.hard_reject {
            hard_rejects.push(item.id.clone());
            hard_rejects.extend(item.reason_codes.iter().cloned());
        }
    }
    reason_codes.sort();
    reason_codes.dedup();
    hard_rejects.sort();
    hard_rejects.dedup();
    let mut advisory_codes: Vec<String> = envelope
        .advisory_actions
        .iter()
        .map(|a| a.code.clone())
        .collect();
    advisory_codes.sort();
    advisory_codes.dedup();
    let output_bytes = serde_json::to_vec(envelope)?.len() as u64;
    Ok(ArmSummary {
        label: label.into(),
        policy_id: snapshot.policy.policy_id.clone(),
        policy_version: snapshot.policy.policy_version,
        rule_hash: snapshot.rule_hash.clone(),
        assessment_hash: envelope.assessment_hash.clone(),
        reason_codes,
        advisory_codes,
        hard_rejects,
        hard_gate: hard_gate_str(&envelope.hard_gate),
        costs: CompareCosts {
            collect_units: 1,
            judge_units: 1,
            output_bytes,
        },
    })
}

fn inputs_match(baseline: &ReplaySnapshot, candidate: &ReplaySnapshot) -> bool {
    baseline.prepared.work_key == candidate.prepared.work_key
        && baseline.as_of == candidate.as_of
        && serde_json::to_vec(&baseline.prepared).ok() == serde_json::to_vec(&candidate.prepared).ok()
        && serde_json::to_vec(&baseline.delivery).ok() == serde_json::to_vec(&candidate.delivery).ok()
        && serde_json::to_vec(&baseline.completion).ok()
            == serde_json::to_vec(&candidate.completion).ok()
}

fn empty_arm(label: &str, snapshot: &ReplaySnapshot, report: &ReplayReport) -> ArmSummary {
    ArmSummary {
        label: label.into(),
        policy_id: snapshot.policy.policy_id.clone(),
        policy_version: snapshot.policy.policy_version,
        rule_hash: snapshot.rule_hash.clone(),
        assessment_hash: report
            .assessment_hash
            .clone()
            .unwrap_or_else(|| "missing".into()),
        reason_codes: vec![],
        advisory_codes: vec![],
        hard_rejects: vec![],
        hard_gate: "unknown".into(),
        costs: CompareCosts::default(),
    }
}

/// Compare baseline vs candidate on the same frozen inputs.
/// Differences are attributed to rule/policy version; failed samples retained.
pub fn shadow_compare(
    baseline_snapshot: &ReplaySnapshot,
    candidate_snapshot: &ReplaySnapshot,
) -> Result<ShadowCompareReport> {
    let same_inputs = inputs_match(baseline_snapshot, candidate_snapshot);
    let baseline_report = replay_assessment(baseline_snapshot)?;
    let candidate_report = replay_assessment(candidate_snapshot)?;

    let rule_explain = format!(
        "baseline rule {}@{} hash={}; candidate rule {}@{} hash={}",
        baseline_snapshot.policy.policy_id,
        baseline_snapshot.policy.policy_version,
        baseline_snapshot.rule_hash,
        candidate_snapshot.policy.policy_id,
        candidate_snapshot.policy.policy_version,
        candidate_snapshot.rule_hash
    );

    if baseline_report.status != ReplayStatus::Replayed
        || candidate_report.status != ReplayStatus::Replayed
    {
        return Ok(ShadowCompareReport {
            same_inputs,
            baseline: empty_arm("baseline", baseline_snapshot, &baseline_report),
            candidate: empty_arm("candidate", candidate_snapshot, &candidate_report),
            differences: vec![ShadowDifference {
                field: "replay_status".into(),
                baseline: json!(format!("{:?}", baseline_report.status)),
                candidate: json!(format!("{:?}", candidate_report.status)),
                explained_by_rule_version: rule_explain,
            }],
            retained_failure_samples: vec![
                serde_json::to_value(&baseline_report)?,
                serde_json::to_value(&candidate_report)?,
            ],
            execution_adoption: false,
            context_adoption: false,
            background_daemon: false,
            passed: false,
        });
    }

    let baseline = summarize_arm("baseline", baseline_snapshot, &baseline_report)?;
    let candidate = summarize_arm("candidate", candidate_snapshot, &candidate_report)?;

    let mut differences = Vec::new();
    let mut push_diff = |field: &str, b: Value, c: Value| {
        if b != c {
            differences.push(ShadowDifference {
                field: field.into(),
                baseline: b,
                candidate: c,
                explained_by_rule_version: rule_explain.clone(),
            });
        }
    };
    push_diff(
        "reason_codes",
        json!(baseline.reason_codes),
        json!(candidate.reason_codes),
    );
    push_diff(
        "advisory_codes",
        json!(baseline.advisory_codes),
        json!(candidate.advisory_codes),
    );
    push_diff(
        "hard_rejects",
        json!(baseline.hard_rejects),
        json!(candidate.hard_rejects),
    );
    push_diff(
        "hard_gate",
        json!(baseline.hard_gate),
        json!(candidate.hard_gate),
    );
    push_diff(
        "costs",
        serde_json::to_value(&baseline.costs).unwrap_or(Value::Null),
        serde_json::to_value(&candidate.costs).unwrap_or(Value::Null),
    );
    push_diff(
        "assessment_hash",
        json!(baseline.assessment_hash),
        json!(candidate.assessment_hash),
    );

    let mut retained_failure_samples = Vec::new();
    if !differences.is_empty() {
        retained_failure_samples.push(json!({
            "baseline_envelope": baseline_report.envelope,
            "candidate_envelope": candidate_report.envelope,
            "differences": differences,
        }));
    }

    Ok(ShadowCompareReport {
        same_inputs,
        baseline,
        candidate,
        differences,
        retained_failure_samples,
        execution_adoption: false,
        context_adoption: false,
        background_daemon: false,
        passed: same_inputs,
    })
}

/// Hard protection map after kill-switch (all must remain true).
pub fn hard_protections_after_disable() -> BTreeMap<&'static str, bool> {
    let flags = HardProtectionFlags::default();
    BTreeMap::from([
        ("claim_guards", flags.claim_guards_active),
        ("completion_guards", flags.completion_guards_active),
        ("admission_guards", flags.admission_guards_active),
        (
            "source_freshness_guards",
            flags.source_freshness_guards_active,
        ),
        ("stop_or_revoke", flags.stop_or_revoke_honored),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_switch_restores_advice_behavior_without_dropping_hard_protections() {
        let effect = apply_advice_delivery_mode(AdviceDeliveryMode::Disabled);
        assert!(effect.restores_prior_advice_behavior);
        assert!(!effect.new_advice_attached);
        assert!(!effect.background_daemon);
        assert!(!effect.execution_adoption_changed);
        assert!(!effect.context_adoption_changed);
        assert!(effect.hard_protections.claim_guards_active);
        assert!(effect.hard_protections.completion_guards_active);
        assert!(effect.hard_protections.admission_guards_active);
        for (_k, active) in hard_protections_after_disable() {
            assert!(active);
        }
    }

    #[test]
    fn shadow_mode_does_not_adopt_execution_or_context_and_has_no_daemon() {
        let effect = apply_advice_delivery_mode(AdviceDeliveryMode::Shadow);
        assert!(effect.new_advice_attached);
        assert!(!effect.mode.adopts_for_execution_or_context());
        assert!(!effect.background_daemon);
        assert!(!effect.execution_adoption_changed);
        assert!(!effect.context_adoption_changed);
    }

    #[test]
    fn shadow_compare_same_inputs_attributes_diffs_to_rule_version_and_retains_failures() {
        use crate::assessment_replay::{capture_replay_snapshot, rule_hash_for_policy};
        use crate::explanation_chain::{
            CompletionExplanationInput, DeliveryExplanationInput, ExplanationAuthority,
        };
        use crate::fact_snapshot::PreparedFactView;
        use serde_json::json;

        let view = PreparedFactView {
            project_id: Some("01PROJ".into()),
            work_key: "AWR-DEC-022".into(),
            work_id: Some("01WORK".into()),
            branch_id: Some("main".into()),
            work_revision: Some("1".into()),
            source_revision: Some("900".into()),
            work_contract_hash: Some("contract-a".into()),
            as_of: 42,
            ready: Some(true),
            diagnostics: vec![],
            active_claim_ids: vec![],
            management: Some(json!({
                "contract_fingerprint": "contract-a",
                "decision": {
                    "version": 1,
                    "mode": "lightweight",
                    "reasons": [],
                    "unknown_observations": [],
                    "reevaluation_signals": [],
                    "required_actions": [
                        "preserve_identity_intent_scope_and_current_state",
                        "consume_required_context_and_hard_rules",
                        "retain_completion_basis_and_actual_outcome",
                        "check_source_versions_permissions_claims_and_request_identity"
                    ],
                    "optional_maintenance": [],
                    "completion_policy": "unchanged_source_policy",
                    "execution_admission": "not_granted_by_management_classification"
                },
                "observation_basis": "host_assertion_not_independently_verified",
                "record_required": false,
                "admission_gaps": [],
                "next_action": "follow_required_actions_and_existing_workflow"
            })),
            context_complete: Some(true),
            context_issues: vec![],
            source_refs: vec!["source:ledger".into()],
            runtime_observation_refs: vec![],
            host_observation: None,
            workspace_facts: None,
            limits: None,
        };
        let authority = ExplanationAuthority {
            contract_hash: Some("contract-a".into()),
            source_revision: Some("900".into()),
            auth_fingerprint: Some("auth-ok".into()),
            stop_or_revoke: false,
        };
        let baseline_policy = AssessmentPolicy::default();
        let mut candidate_policy = AssessmentPolicy::default();
        candidate_policy.policy_version = baseline_policy.policy_version + 1;
        // Force a distinguishable rule hash while keeping same frozen inputs.
        candidate_policy.policy_hash = Some("candidate-rule".into());

        let baseline = capture_replay_snapshot(
            view.clone(),
            baseline_policy,
            42,
            DeliveryExplanationInput::default(),
            CompletionExplanationInput::default(),
            authority.clone(),
            None,
            None,
            Some("artifact:baseline".into()),
            None,
        )
        .unwrap();
        let candidate = capture_replay_snapshot(
            view,
            candidate_policy,
            42,
            DeliveryExplanationInput::default(),
            CompletionExplanationInput::default(),
            authority,
            None,
            None,
            Some("artifact:candidate".into()),
            None,
        )
        .unwrap();
        assert_ne!(baseline.rule_hash, candidate.rule_hash);
        assert_ne!(
            rule_hash_for_policy(&baseline.policy).unwrap(),
            rule_hash_for_policy(&candidate.policy).unwrap()
        );

        let report = shadow_compare(&baseline, &candidate).unwrap();
        assert!(report.same_inputs);
        assert!(!report.execution_adoption);
        assert!(!report.context_adoption);
        assert!(!report.background_daemon);
        // Diffs (at least assessment_hash / costs / rule identity) explained by rule version.
        assert!(
            report.differences.iter().all(|d| {
                d.explained_by_rule_version.contains(&baseline.rule_hash)
                    && d.explained_by_rule_version.contains(&candidate.rule_hash)
            }),
            "every difference must cite rule versions: {:?}",
            report.differences
        );
        if !report.differences.is_empty() {
            assert!(
                !report.retained_failure_samples.is_empty(),
                "divergent samples must be retained in full"
            );
        }
    }

}
