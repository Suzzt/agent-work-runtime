//! Context gaps for selective invalidation and scoped planning changes (WS-032).
//!
//! Context packets must never look falsely ready when a consumer edge needs
//! re-evaluation or when an open planning change blocks affected actions.
//! Unrelated works are omitted from blocked sets so they keep progressing.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Why a work must not present as ready in a compiled context packet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessBlockReason {
    SelectiveReevaluationRequired,
    PlanningChangePendingConfirmation,
    BoundaryRevalidationFailed,
    MidExecutionRecoveryDuty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessBlock {
    pub work_id: String,
    pub reason: ReadinessBlockReason,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvalidationContextGaps {
    pub subject_work_id: String,
    pub blocks: Vec<ReadinessBlock>,
    /// Works that remain free to progress under the same project snapshot.
    pub unrelated_continuing: Vec<String>,
    pub falsely_complete: bool,
}

/// Build context gaps from selective re-evaluation + open planning blocks.
///
/// `reevaluate_work_ids` come from selective invalidation; `planning_blocked`
/// pairs `(work_id, change_id)` for AffectedBlocked planning changes;
/// `recovery_duty_work_ids` are mid-execution invalidations that retained effects.
pub fn invalidation_context_gaps(
    subject_work_id: &str,
    reevaluate_work_ids: &[String],
    planning_blocked: &[(String, String)],
    recovery_duty_work_ids: &[String],
    all_project_work_ids: &[String],
) -> InvalidationContextGaps {
    let mut blocks = Vec::new();
    let mut blocked: BTreeSet<String> = BTreeSet::new();

    if reevaluate_work_ids.iter().any(|w| w == subject_work_id) {
        blocks.push(ReadinessBlock {
            work_id: subject_work_id.into(),
            reason: ReadinessBlockReason::SelectiveReevaluationRequired,
            reference: format!("selective:{subject_work_id}"),
        });
        blocked.insert(subject_work_id.into());
    }

    for (work_id, change_id) in planning_blocked {
        if work_id == subject_work_id {
            blocks.push(ReadinessBlock {
                work_id: work_id.clone(),
                reason: ReadinessBlockReason::PlanningChangePendingConfirmation,
                reference: format!("planning_change:{change_id}"),
            });
            blocked.insert(work_id.clone());
        }
    }

    for w in recovery_duty_work_ids {
        if w == subject_work_id {
            blocks.push(ReadinessBlock {
                work_id: w.clone(),
                reason: ReadinessBlockReason::MidExecutionRecoveryDuty,
                reference: format!("recovery:{w}"),
            });
            blocked.insert(w.clone());
        }
    }

    let unrelated_continuing: Vec<String> = all_project_work_ids
        .iter()
        .filter(|w| !blocked.contains(w.as_str()) && w.as_str() != subject_work_id)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let falsely_complete = !blocks.is_empty();
    InvalidationContextGaps {
        subject_work_id: subject_work_id.into(),
        blocks,
        unrelated_continuing,
        falsely_complete,
    }
}

/// Merge invalidation gaps into completeness: any block means not ready.
pub fn readiness_complete_with_gaps(gaps: &InvalidationContextGaps) -> bool {
    gaps.blocks.is_empty() && !gaps.falsely_complete
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_block_leaves_unrelated_free() {
        let gaps = invalidation_context_gaps(
            "sdk",
            &["sdk".into()],
            &[("sdk".into(), "pc-1".into())],
            &[],
            &["api".into(), "sdk".into(), "docs".into()],
        );
        assert!(gaps.falsely_complete);
        assert_eq!(gaps.blocks.len(), 2);
        assert_eq!(
            gaps.unrelated_continuing,
            vec!["api".to_string(), "docs".to_string()]
        );
        assert!(!readiness_complete_with_gaps(&gaps));
    }

    #[test]
    fn unrelated_subject_has_no_blocks() {
        let gaps = invalidation_context_gaps(
            "docs",
            &["sdk".into()],
            &[("sdk".into(), "pc-1".into())],
            &["sdk".into()],
            &["api".into(), "sdk".into(), "docs".into()],
        );
        assert!(gaps.blocks.is_empty());
        assert!(!gaps.falsely_complete);
        assert!(readiness_complete_with_gaps(&gaps));
    }
}
