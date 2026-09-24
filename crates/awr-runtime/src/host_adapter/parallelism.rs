//! Controlled subtask parallelism under a parent session (WS-024).
use awr_core::{ParentRollupRef, Result, SubtaskIdentity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtaskState {
    Pending,
    Ready,
    Running,
    Paused,
    AwaitingHandoff,
    AwaitingAcceptance,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtaskRecord {
    pub identity: SubtaskIdentity,
    pub state: SubtaskState,
    pub execution_id: Option<String>,
    pub outcome_ref: Option<String>,
    /// Read-only helper calls remain execution detail (no separate claim).
    #[serde(default)]
    pub internal_readonly_helpers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleDecision {
    Start,
    WaitDependencies,
    BlockedByConcurrencyCap,
    BlockedByPause,
    SkipTerminal,
    ReconnectExisting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParallelDispatchPlan {
    pub start: Vec<String>,
    pub wait_dependencies: Vec<String>,
    pub blocked_by_cap: Vec<String>,
    pub blocked_by_pause: Vec<String>,
    pub reconnect: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseGate {
    Running,
    UserPaused,
}

/// Effect of ending the parent session on children.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentExitEffect {
    pub parent_session_id: String,
    pub children_auto_completed: bool,
    pub unknown_children_released: bool,
    pub retained_child_work_ids: Vec<String>,
    pub note: String,
}

/// Reconnect-before-retry: query original execution first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconnectRetry {
    pub work_id: String,
    pub original_execution_id: String,
    pub duplicate_start_allowed: bool,
    pub action: String,
}

fn consumes_concurrency_capacity(task: &SubtaskRecord) -> bool {
    // Loss of observation or unresolved handoff cannot free capacity: the native
    // execution may still be running without verified terminal/stop evidence.
    task.execution_id.is_some()
        && matches!(
            task.state,
            SubtaskState::Running
                | SubtaskState::Paused
                | SubtaskState::Unknown
                | SubtaskState::AwaitingHandoff
        )
}

fn unresolved_existing_execution(task: &SubtaskRecord) -> bool {
    consumes_concurrency_capacity(task)
}

pub struct ParallelScheduler {
    parent_session_id: String,
    concurrency_cap: usize,
    pause: PauseGate,
    tasks: BTreeMap<String, SubtaskRecord>,
}

impl ParallelScheduler {
    pub fn new(parent_session_id: impl Into<String>, concurrency_cap: usize) -> Result<Self> {
        let parent_session_id = parent_session_id.into();
        if parent_session_id.trim().is_empty() || parent_session_id.len() > 128 {
            return Err(awr_core::Error::InvalidInput(
                "parent_session_id must be bounded".into(),
            ));
        }
        if !(1..=64).contains(&concurrency_cap) {
            return Err(awr_core::Error::InvalidInput(
                "concurrency_cap must be 1..=64".into(),
            ));
        }
        Ok(Self {
            parent_session_id,
            concurrency_cap,
            pause: PauseGate::Running,
            tasks: BTreeMap::new(),
        })
    }

    pub fn parent_session_id(&self) -> &str {
        &self.parent_session_id
    }

    pub fn concurrency_cap(&self) -> usize {
        self.concurrency_cap
    }

    pub fn pause_gate(&self) -> PauseGate {
        self.pause
    }

    pub fn set_pause(&mut self, pause: PauseGate) {
        self.pause = pause;
        if pause == PauseGate::UserPaused {
            for task in self.tasks.values_mut() {
                if task.state == SubtaskState::Running {
                    task.state = SubtaskState::Paused;
                }
            }
        } else {
            for task in self.tasks.values_mut() {
                if task.state == SubtaskState::Paused {
                    task.state = SubtaskState::Running;
                }
            }
        }
    }

    pub fn register(&mut self, identity: SubtaskIdentity) -> Result<()> {
        identity.validate()?;
        if identity.parent_session_id != self.parent_session_id {
            return Err(awr_core::Error::InvalidInput(
                "subtask parent_session_id mismatch".into(),
            ));
        }
        if self.tasks.contains_key(&identity.work_id) {
            return Err(awr_core::Error::InvalidInput(
                "subtask work_id already registered".into(),
            ));
        }
        // Resource bounds must not trivially collide on identical shared keys
        // within the same parent schedule when both are active.
        for existing in self.tasks.values() {
            if resources_overlap(
                &existing.identity.resource_bounds,
                &identity.resource_bounds,
            ) && !matches!(
                existing.state,
                SubtaskState::Succeeded | SubtaskState::Failed
            ) {
                return Err(awr_core::Error::InvalidInput(
                    "subtask resource bounds conflict with an active sibling".into(),
                ));
            }
        }
        let state = if identity.depends_on.is_empty() {
            SubtaskState::Ready
        } else {
            SubtaskState::Pending
        };
        self.tasks.insert(
            identity.work_id.clone(),
            SubtaskRecord {
                identity,
                state,
                execution_id: None,
                outcome_ref: None,
                internal_readonly_helpers: Vec::new(),
            },
        );
        Ok(())
    }

    pub fn mark_awaiting_handoff(&mut self, work_id: &str) -> Result<()> {
        let task = self
            .tasks
            .get_mut(work_id)
            .ok_or_else(|| awr_core::Error::InvalidInput("unknown subtask".into()))?;
        task.state = SubtaskState::AwaitingHandoff;
        Ok(())
    }

    pub fn mark_awaiting_acceptance(&mut self, work_id: &str) -> Result<()> {
        let task = self
            .tasks
            .get_mut(work_id)
            .ok_or_else(|| awr_core::Error::InvalidInput("unknown subtask".into()))?;
        task.state = SubtaskState::AwaitingAcceptance;
        Ok(())
    }

    pub fn attach_internal_helper(
        &mut self,
        work_id: &str,
        helper: impl Into<String>,
    ) -> Result<()> {
        let task = self
            .tasks
            .get_mut(work_id)
            .ok_or_else(|| awr_core::Error::InvalidInput("unknown subtask".into()))?;
        let helper = helper.into();
        if helper.trim().is_empty() || helper.len() > 256 {
            return Err(awr_core::Error::InvalidInput(
                "helper name must be bounded".into(),
            ));
        }
        task.internal_readonly_helpers.push(helper);
        Ok(())
    }

    pub fn decide(&self, work_id: &str) -> Result<ScheduleDecision> {
        let task = self
            .tasks
            .get(work_id)
            .ok_or_else(|| awr_core::Error::InvalidInput("unknown subtask".into()))?;
        if matches!(
            task.state,
            SubtaskState::Succeeded | SubtaskState::Failed | SubtaskState::AwaitingAcceptance
        ) {
            return Ok(ScheduleDecision::SkipTerminal);
        }
        // Block/reconnect any unresolved existing execution regardless of
        // coordination state (including AwaitingHandoff) — do not overwrite.
        if unresolved_existing_execution(task) {
            return Ok(ScheduleDecision::ReconnectExisting);
        }
        if self.pause == PauseGate::UserPaused {
            return Ok(ScheduleDecision::BlockedByPause);
        }
        if !self.dependencies_satisfied(task) {
            return Ok(ScheduleDecision::WaitDependencies);
        }
        let occupied = self
            .tasks
            .values()
            .filter(|t| consumes_concurrency_capacity(t))
            .count();
        if occupied >= self.concurrency_cap {
            return Ok(ScheduleDecision::BlockedByConcurrencyCap);
        }
        Ok(ScheduleDecision::Start)
    }

    pub fn plan(&self) -> ParallelDispatchPlan {
        let mut plan = ParallelDispatchPlan {
            start: Vec::new(),
            wait_dependencies: Vec::new(),
            blocked_by_cap: Vec::new(),
            blocked_by_pause: Vec::new(),
            reconnect: Vec::new(),
        };
        // Deterministic work_id order.
        let mut remaining_slots = self.concurrency_cap.saturating_sub(
            self.tasks
                .values()
                .filter(|t| consumes_concurrency_capacity(t))
                .count(),
        );
        for work_id in self.tasks.keys() {
            match self
                .decide(work_id)
                .unwrap_or(ScheduleDecision::SkipTerminal)
            {
                ScheduleDecision::Start => {
                    if remaining_slots > 0 && self.pause == PauseGate::Running {
                        plan.start.push(work_id.clone());
                        remaining_slots -= 1;
                    } else if self.pause == PauseGate::UserPaused {
                        plan.blocked_by_pause.push(work_id.clone());
                    } else {
                        plan.blocked_by_cap.push(work_id.clone());
                    }
                }
                ScheduleDecision::WaitDependencies => plan.wait_dependencies.push(work_id.clone()),
                ScheduleDecision::BlockedByConcurrencyCap => {
                    plan.blocked_by_cap.push(work_id.clone())
                }
                ScheduleDecision::BlockedByPause => plan.blocked_by_pause.push(work_id.clone()),
                ScheduleDecision::ReconnectExisting => plan.reconnect.push(work_id.clone()),
                ScheduleDecision::SkipTerminal => {}
            }
        }
        plan
    }

    pub fn start(&mut self, work_id: &str, execution_id: impl Into<String>) -> Result<()> {
        match self.decide(work_id)? {
            ScheduleDecision::Start => {}
            ScheduleDecision::ReconnectExisting => {
                return Err(awr_core::Error::InvalidInput(
                    "execution already exists; reconnect before starting a duplicate".into(),
                ));
            }
            other => {
                return Err(awr_core::Error::InvalidInput(format!(
                    "cannot start subtask: {other:?}"
                )));
            }
        }
        let task = self.tasks.get_mut(work_id).expect("checked");
        task.execution_id = Some(execution_id.into());
        task.state = SubtaskState::Running;
        Ok(())
    }

    pub fn complete(
        &mut self,
        work_id: &str,
        success: bool,
        outcome_ref: impl Into<String>,
    ) -> Result<ParentRollupRef> {
        let task = self
            .tasks
            .get_mut(work_id)
            .ok_or_else(|| awr_core::Error::InvalidInput("unknown subtask".into()))?;
        let execution_id = task.execution_id.clone().ok_or_else(|| {
            awr_core::Error::InvalidInput("subtask has no execution to complete".into())
        })?;
        let outcome_ref = outcome_ref.into();
        task.outcome_ref = Some(outcome_ref.clone());
        task.state = if success {
            SubtaskState::Succeeded
        } else {
            SubtaskState::Failed
        };
        ParentRollupRef::reference(
            self.parent_session_id.clone(),
            work_id,
            execution_id,
            outcome_ref,
        )
    }

    pub fn mark_unknown(&mut self, work_id: &str) -> Result<()> {
        let task = self
            .tasks
            .get_mut(work_id)
            .ok_or_else(|| awr_core::Error::InvalidInput("unknown subtask".into()))?;
        task.state = SubtaskState::Unknown;
        Ok(())
    }

    /// Parent session exit does not auto-complete or release unknown children.
    pub fn parent_session_exit(&self) -> ParentExitEffect {
        let retained: Vec<String> = self
            .tasks
            .values()
            .filter(|t| !matches!(t.state, SubtaskState::Succeeded | SubtaskState::Failed))
            .map(|t| t.identity.work_id.clone())
            .collect();
        ParentExitEffect {
            parent_session_id: self.parent_session_id.clone(),
            children_auto_completed: false,
            unknown_children_released: false,
            retained_child_work_ids: retained,
            note: "Parent session exit retains child executions and claims; operators must reconnect or explicitly finish each child.".into(),
        }
    }

    /// Reconnect/retry queries the original execution first — no duplicate side effects.
    pub fn reconnect_before_retry(&self, work_id: &str) -> Result<ReconnectRetry> {
        let task = self
            .tasks
            .get(work_id)
            .ok_or_else(|| awr_core::Error::InvalidInput("unknown subtask".into()))?;
        let Some(execution_id) = task.execution_id.clone() else {
            return Ok(ReconnectRetry {
                work_id: work_id.into(),
                original_execution_id: String::new(),
                duplicate_start_allowed: true,
                action: "no_original_execution_start_allowed".into(),
            });
        };
        Ok(ReconnectRetry {
            work_id: work_id.into(),
            original_execution_id: execution_id,
            duplicate_start_allowed: false,
            action: "query_original_execution_then_reconnect".into(),
        })
    }

    pub fn get(&self, work_id: &str) -> Option<&SubtaskRecord> {
        self.tasks.get(work_id)
    }

    pub fn tasks(&self) -> impl Iterator<Item = &SubtaskRecord> {
        self.tasks.values()
    }

    fn dependencies_satisfied(&self, task: &SubtaskRecord) -> bool {
        task.identity.depends_on.iter().all(|dep| {
            self.tasks
                .get(dep)
                .is_some_and(|d| d.state == SubtaskState::Succeeded)
        })
    }
}

fn resources_overlap(
    a: &[awr_core::SubtaskResourceBound],
    b: &[awr_core::SubtaskResourceBound],
) -> bool {
    for x in a {
        for y in b {
            if x.kind == y.kind && x.key == y.key && x.worktree_id == y.worktree_id {
                return true;
            }
            // Shared kinds conflict by key regardless of worktree (must be empty).
            if matches!(x.kind.as_str(), "external" | "integration" | "named")
                && x.kind == y.kind
                && x.key == y.key
            {
                return true;
            }
        }
    }
    false
}

/// Collect parent rollup references without copying artifacts.
pub fn rollup_refs(scheduler: &ParallelScheduler) -> Result<Vec<ParentRollupRef>> {
    let mut out = Vec::new();
    for task in scheduler.tasks() {
        if let (Some(exec), Some(outcome)) = (&task.execution_id, &task.outcome_ref) {
            out.push(ParentRollupRef::reference(
                scheduler.parent_session_id(),
                task.identity.work_id.clone(),
                exec.clone(),
                outcome.clone(),
            )?);
        }
    }
    // Guard: never allow any copied flag.
    if out.iter().any(|r| r.artifacts_copied) {
        return Err(awr_core::Error::InvalidInput(
            "parent rollup must not copy child artifacts".into(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use awr_core::SubtaskResourceBound;

    fn identity(work: &str, claim: &str, child_sess: &str, deps: &[&str]) -> SubtaskIdentity {
        SubtaskIdentity {
            work_id: work.into(),
            claim_id: claim.into(),
            parent_session_id: "parent-sess".into(),
            child_session_id: child_sess.into(),
            agent_label: format!("agent-{work}"),
            resource_bounds: vec![SubtaskResourceBound {
                kind: "workspace".into(),
                key: format!("wt/{work}"),
                worktree_id: work.into(),
            }],
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn independent_tasks_start_in_parallel_under_cap() {
        let mut s = ParallelScheduler::new("parent-sess", 2).unwrap();
        s.register(identity("a", "c-a", "child-a", &[])).unwrap();
        s.register(identity("b", "c-b", "child-b", &[])).unwrap();
        s.register(identity("c", "c-c", "child-c", &[])).unwrap();
        let plan = s.plan();
        assert_eq!(plan.start, vec!["a", "b"]);
        assert_eq!(plan.blocked_by_cap, vec!["c"]);
    }

    #[test]
    fn dependencies_order_start() {
        let mut s = ParallelScheduler::new("parent-sess", 4).unwrap();
        s.register(identity("a", "c-a", "child-a", &[])).unwrap();
        s.register(identity("b", "c-b", "child-b", &["a"])).unwrap();
        let plan = s.plan();
        assert_eq!(plan.start, vec!["a"]);
        assert_eq!(plan.wait_dependencies, vec!["b"]);
        s.start("a", "exec-a").unwrap();
        s.complete("a", true, "ref://a").unwrap();
        let plan = s.plan();
        assert_eq!(plan.start, vec!["b"]);
    }

    #[test]
    fn pause_blocks_new_starts() {
        let mut s = ParallelScheduler::new("parent-sess", 2).unwrap();
        s.register(identity("a", "c-a", "child-a", &[])).unwrap();
        s.set_pause(PauseGate::UserPaused);
        assert_eq!(s.decide("a").unwrap(), ScheduleDecision::BlockedByPause);
    }

    #[test]
    fn parent_exit_retains_unknown_children() {
        let mut s = ParallelScheduler::new("parent-sess", 2).unwrap();
        s.register(identity("a", "c-a", "child-a", &[])).unwrap();
        s.start("a", "exec-a").unwrap();
        s.mark_unknown("a").unwrap();
        let effect = s.parent_session_exit();
        assert!(!effect.children_auto_completed);
        assert!(!effect.unknown_children_released);
        assert_eq!(effect.retained_child_work_ids, vec!["a"]);
    }

    #[test]
    fn reconnect_before_retry_forbids_duplicate_start() {
        let mut s = ParallelScheduler::new("parent-sess", 2).unwrap();
        s.register(identity("a", "c-a", "child-a", &[])).unwrap();
        s.start("a", "exec-a").unwrap();
        let retry = s.reconnect_before_retry("a").unwrap();
        assert!(!retry.duplicate_start_allowed);
        assert_eq!(retry.original_execution_id, "exec-a");
        assert!(s.start("a", "exec-dup").is_err());
    }

    #[test]
    fn rollup_does_not_copy_artifacts() {
        let mut s = ParallelScheduler::new("parent-sess", 2).unwrap();
        s.register(identity("a", "c-a", "child-a", &[])).unwrap();
        s.start("a", "exec-a").unwrap();
        let rollup = s.complete("a", true, "ref://artifact-a").unwrap();
        assert!(!rollup.artifacts_copied);
        let refs = rollup_refs(&s).unwrap();
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn awaiting_subtasks_keep_identity_and_claims() {
        let mut s = ParallelScheduler::new("parent-sess", 2).unwrap();
        s.register(identity("a", "claim-a", "child-a", &[]))
            .unwrap();
        s.mark_awaiting_handoff("a").unwrap();
        let task = s.get("a").unwrap();
        assert_eq!(task.identity.claim_id, "claim-a");
        assert!(!task.identity.resource_bounds.is_empty());
        assert_eq!(task.state, SubtaskState::AwaitingHandoff);
    }

    #[test]
    fn internal_helpers_do_not_require_new_claim() {
        let mut s = ParallelScheduler::new("parent-sess", 2).unwrap();
        s.register(identity("a", "claim-a", "child-a", &[]))
            .unwrap();
        s.attach_internal_helper("a", "readonly_context_compile")
            .unwrap();
        assert_eq!(
            s.get("a").unwrap().internal_readonly_helpers,
            vec!["readonly_context_compile"]
        );
        // Still one claim identity.
        assert_eq!(s.get("a").unwrap().identity.claim_id, "claim-a");
    }

    #[test]
    fn awaiting_handoff_preserves_original_execution() {
        let mut s = ParallelScheduler::new("parent-sess", 2).unwrap();
        s.register(identity("a", "c-a", "child-a", &[])).unwrap();
        s.start("a", "exec-original").unwrap();
        s.mark_awaiting_handoff("a").unwrap();
        assert_eq!(s.decide("a").unwrap(), ScheduleDecision::ReconnectExisting);
        assert!(s.start("a", "exec-duplicate").is_err());
        assert_eq!(
            s.get("a").unwrap().execution_id.as_deref(),
            Some("exec-original")
        );
    }

    #[test]
    fn unknown_executions_consume_concurrency_capacity() {
        let mut s = ParallelScheduler::new("parent-sess", 1).unwrap();
        s.register(identity("a", "c-a", "child-a", &[])).unwrap();
        s.register(identity("b", "c-b", "child-b", &[])).unwrap();
        s.start("a", "exec-a").unwrap();
        s.mark_unknown("a").unwrap();
        assert_eq!(
            s.decide("b").unwrap(),
            ScheduleDecision::BlockedByConcurrencyCap
        );
        assert!(s.start("b", "exec-b").is_err());
        let plan = s.plan();
        assert!(plan.start.is_empty());
        assert!(plan.blocked_by_cap.contains(&"b".into()) || plan.reconnect.contains(&"a".into()));
    }
}
