//! Confirmed Team long-term handoff (WS-017).
//!
//! **Execution handoff** and **responsibility transfer** are separate operations.
//! State machine: propose → inspect → accept | reject | cancel | timeout.
//! Timeout does not stop execution; disconnect / reject / no-receiver keep the
//! original responsibility and recovery duty. Accept is transactional and grants
//! successor execution only after the prior execution is stopped or reconciled.
//! Exactly one concurrent accept wins; late writes under an old fence cannot
//! become the current result. Receiver must re-prepare current context.
use crate::{Error, ExecutionInstance, PersonId, Result};

fn validate_id(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(Error::InvalidInput(format!(
            "{field} must be 1..=128 chars without controls"
        )));
    }
    if value.chars().any(|c| c.is_whitespace()) {
        return Err(Error::InvalidInput(format!(
            "{field} must not contain whitespace"
        )));
    }
    Ok(())
}
use serde::{Deserialize, Serialize};

fn validate_request_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 128 || key.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "request_key must be 1..=128 chars without controls".into(),
        ));
    }
    Ok(())
}

fn validate_digest(label: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::InvalidInput(format!(
            "{label} must be 64 lowercase hex chars"
        )));
    }
    Ok(())
}

/// Separates who may continue executing from who owns the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffKind {
    /// Transfer current executor / admission rights only. Ownership unchanged.
    Execution,
    /// Transfer sole ownership (requires separate accept). Never grants execution alone.
    Responsibility,
}

/// Lifecycle of a confirmed handoff proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    Proposed,
    Inspected,
    Accepted,
    Rejected,
    Cancelled,
    TimedOut,
}

impl HandoffStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::Rejected | Self::Cancelled | Self::TimedOut
        )
    }

    pub fn is_open(self) -> bool {
        matches!(self, Self::Proposed | Self::Inspected)
    }
}

/// Who is responsible and who may continue work under each status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffDuty {
    pub responsible_person_id: PersonId,
    pub may_continue_person_id: PersonId,
    pub recovery_duty_person_id: PersonId,
    pub successor_may_execute: bool,
    pub context_requires_refresh: bool,
    pub note: String,
}

/// Package the receiver must consume before accepting. Chat summaries are not enough.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffPackage {
    pub task_id: String,
    pub contract_version: String,
    pub contract_hash: String,
    pub current_person_id: PersonId,
    pub current_execution: ExecutionInstance,
    pub consumed_context_digest: String,
    pub checkpoint_ids: Vec<String>,
    pub artifact_versions: Vec<ArtifactVersionRef>,
    pub branch_id: Option<String>,
    pub working_directory: Option<String>,
    pub dependency_ids: Vec<String>,
    pub todos: Vec<String>,
    pub awaiting_replies: Vec<String>,
    pub unknown_side_effects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactVersionRef {
    pub artifact_id: String,
    pub version: String,
}

impl HandoffPackage {
    pub fn validate(&self) -> Result<()> {
        validate_id(&self.task_id, "task_id")?;
        if self.contract_version.is_empty() || self.contract_version.len() > 128 {
            return Err(Error::InvalidInput(
                "contract_version required (1..=128)".into(),
            ));
        }
        validate_digest("contract_hash", &self.contract_hash)?;
        validate_id(self.current_person_id.as_str(), "current_person_id")?;
        self.current_execution.validate()?;
        if self.current_execution.person_id() != &self.current_person_id {
            return Err(Error::InvalidInput(
                "package current_execution must match current_person_id".into(),
            ));
        }
        validate_digest("consumed_context_digest", &self.consumed_context_digest)?;
        if self.checkpoint_ids.is_empty() {
            return Err(Error::InvalidInput(
                "handoff package requires at least one checkpoint".into(),
            ));
        }
        if self.checkpoint_ids.len() > 64
            || self.artifact_versions.len() > 128
            || self.dependency_ids.len() > 128
            || self.todos.len() > 64
            || self.awaiting_replies.len() > 64
            || self.unknown_side_effects.len() > 64
        {
            return Err(Error::InvalidInput(
                "handoff package collection bounds exceeded".into(),
            ));
        }
        for id in &self.checkpoint_ids {
            validate_id(id, "checkpoint_id")?;
        }
        for a in &self.artifact_versions {
            validate_id(&a.artifact_id, "artifact_id")?;
            if a.version.is_empty() || a.version.len() > 128 {
                return Err(Error::InvalidInput("artifact version required".into()));
            }
        }
        for id in &self.dependency_ids {
            validate_id(id, "dependency_id")?;
        }
        for t in self
            .todos
            .iter()
            .chain(self.awaiting_replies.iter())
            .chain(self.unknown_side_effects.iter())
        {
            if t.trim().is_empty() || t.len() > 4096 || t.contains('\0') {
                return Err(Error::InvalidInput(
                    "todo/awaiting/unknown entries must be nonempty <=4096 without NUL".into(),
                ));
            }
        }
        if let Some(branch) = &self.branch_id {
            validate_id(branch, "branch_id")?;
        }
        if let Some(dir) = &self.working_directory {
            if dir.is_empty() || dir.len() > 4096 || dir.contains('\0') {
                return Err(Error::InvalidInput("working_directory invalid".into()));
            }
        }
        Ok(())
    }
}

/// Durable handoff record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamHandoff {
    pub id: String,
    pub project_id: String,
    pub work_item_id: String,
    pub kind: HandoffKind,
    pub status: HandoffStatus,
    pub version: u64,
    pub package: HandoffPackage,
    pub from_person_id: PersonId,
    pub to_person_id: PersonId,
    pub proposed_successor: Option<ExecutionInstance>,
    pub accepted_successor: Option<ExecutionInstance>,
    pub proposer_execution_id: Option<String>,
    pub proposer_fence: Option<i64>,
    pub expires_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub inspected_at_ms: Option<i64>,
    pub terminal_at_ms: Option<i64>,
    pub terminal_reason: Option<String>,
    pub accept_request_key: Option<String>,
}

impl TeamHandoff {
    pub fn duty_at(&self, now_ms: i64) -> Result<HandoffDuty> {
        let _ = now_ms;
        match self.status {
            HandoffStatus::Proposed | HandoffStatus::Inspected => Ok(HandoffDuty {
                responsible_person_id: self.from_person_id.clone(),
                may_continue_person_id: self.from_person_id.clone(),
                recovery_duty_person_id: self.from_person_id.clone(),
                successor_may_execute: false,
                context_requires_refresh: true,
                note: "proposal open; original retains responsibility and recovery; receiver must re-prepare before accept".into(),
            }),
            HandoffStatus::Accepted => match self.kind {
                HandoffKind::Execution => {
                    let successor = self
                        .accepted_successor
                        .as_ref()
                        .map(|e| e.person_id().clone())
                        .unwrap_or_else(|| self.to_person_id.clone());
                    Ok(HandoffDuty {
                        responsible_person_id: self.from_person_id.clone(),
                        may_continue_person_id: successor.clone(),
                        recovery_duty_person_id: successor,
                        successor_may_execute: true,
                        context_requires_refresh: true,
                        note: "execution handoff accepted; ownership unchanged; successor may continue after re-prepare".into(),
                    })
                }
                HandoffKind::Responsibility => Ok(HandoffDuty {
                    responsible_person_id: self.to_person_id.clone(),
                    may_continue_person_id: self.to_person_id.clone(),
                    recovery_duty_person_id: self.to_person_id.clone(),
                    successor_may_execute: false,
                    context_requires_refresh: true,
                    note: "responsibility transferred; execution admission requires a separate execution handoff or claim".into(),
                }),
            },
            HandoffStatus::Rejected | HandoffStatus::Cancelled | HandoffStatus::TimedOut => {
                Ok(HandoffDuty {
                    responsible_person_id: self.from_person_id.clone(),
                    may_continue_person_id: self.from_person_id.clone(),
                    recovery_duty_person_id: self.from_person_id.clone(),
                    successor_may_execute: false,
                    context_requires_refresh: false,
                    note: "handoff closed without transfer; original keeps responsibility and recovery duty; timeout does not stop execution".into(),
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposeHandoffRequest {
    pub request_key: String,
    pub handoff_id: String,
    pub kind: HandoffKind,
    pub package: HandoffPackage,
    pub to_person_id: PersonId,
    pub proposed_successor: Option<ExecutionInstance>,
    pub proposer_execution_id: Option<String>,
    pub proposer_fence: Option<i64>,
    pub expires_at_ms: Option<i64>,
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectHandoffRequest {
    pub request_key: String,
    pub handoff_id: String,
    pub expected_version: u64,
    pub inspector_person_id: PersonId,
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptHandoffRequest {
    pub request_key: String,
    pub handoff_id: String,
    pub expected_version: u64,
    pub acceptor_person_id: PersonId,
    pub successor_execution: ExecutionInstance,
    /// Prior execution stopped (claim released / session ended) OR results reconciled.
    pub prior_execution_stopped: bool,
    pub prior_reconciled: bool,
    /// Receiver must affirm current context was re-prepared (not chat trust).
    pub context_reprepared: bool,
    /// Fence the acceptor believes is current; old fence cannot win.
    pub expected_current_fence: Option<i64>,
    pub live_fence: Option<i64>,
    pub unknown_executions_open: bool,
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectHandoffRequest {
    pub request_key: String,
    pub handoff_id: String,
    pub expected_version: u64,
    pub by_person_id: PersonId,
    pub reason: String,
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelHandoffRequest {
    pub request_key: String,
    pub handoff_id: String,
    pub expected_version: u64,
    pub by_person_id: PersonId,
    pub reason: String,
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeoutHandoffRequest {
    pub request_key: String,
    pub handoff_id: String,
    pub expected_version: u64,
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffReceipt {
    pub request_key: String,
    pub handoff_id: String,
    pub op: String,
    pub event_id: String,
    pub replayed: bool,
    pub status: HandoffStatus,
    pub version: u64,
    pub duty: HandoffDuty,
}

pub fn apply_handoff_propose(
    project_id: &str,
    work_item_id: &str,
    from_person: &PersonId,
    req: &ProposeHandoffRequest,
) -> Result<TeamHandoff> {
    validate_request_key(&req.request_key)?;
    validate_id(&req.handoff_id, "handoff_id")?;
    validate_id(project_id, "project_id")?;
    validate_id(work_item_id, "work_item_id")?;
    req.package.validate()?;
    if req.package.task_id != work_item_id {
        return Err(Error::InvalidInput(
            "package.task_id must match work item".into(),
        ));
    }
    if &req.package.current_person_id != from_person {
        return Err(Error::RuleViolation(
            "proposer must be the package current person".into(),
        ));
    }
    if &req.to_person_id == from_person && req.kind == HandoffKind::Responsibility {
        return Err(Error::InvalidInput(
            "responsibility transfer requires a different person".into(),
        ));
    }
    // Same-person agent swap is an execution handoff with a different AgentRun.
    if let Some(successor) = &req.proposed_successor {
        successor.validate()?;
        match req.kind {
            HandoffKind::Execution => {
                if successor.person_id() != &req.to_person_id {
                    return Err(Error::InvalidInput(
                        "proposed successor person must match to_person_id".into(),
                    ));
                }
            }
            HandoffKind::Responsibility => {
                if successor.person_id() != &req.to_person_id {
                    return Err(Error::InvalidInput(
                        "responsibility successor must be the receiving person".into(),
                    ));
                }
            }
        }
    }
    if let Some(exp) = req.expires_at_ms {
        if exp <= req.now_ms {
            return Err(Error::InvalidInput(
                "expires_at_ms must be in the future".into(),
            ));
        }
    }
    Ok(TeamHandoff {
        id: req.handoff_id.clone(),
        project_id: project_id.into(),
        work_item_id: work_item_id.into(),
        kind: req.kind,
        status: HandoffStatus::Proposed,
        version: 1,
        package: req.package.clone(),
        from_person_id: from_person.clone(),
        to_person_id: req.to_person_id.clone(),
        proposed_successor: req.proposed_successor.clone(),
        accepted_successor: None,
        proposer_execution_id: req.proposer_execution_id.clone(),
        proposer_fence: req.proposer_fence,
        expires_at_ms: req.expires_at_ms,
        created_at_ms: req.now_ms,
        updated_at_ms: req.now_ms,
        inspected_at_ms: None,
        terminal_at_ms: None,
        terminal_reason: None,
        accept_request_key: None,
    })
}

pub fn apply_handoff_inspect(
    current: &TeamHandoff,
    req: &InspectHandoffRequest,
) -> Result<TeamHandoff> {
    validate_request_key(&req.request_key)?;
    require_version(current, req.expected_version)?;
    if current.id != req.handoff_id {
        return Err(Error::NotFound("handoff id mismatch".into()));
    }
    if !current.status.is_open() {
        return Err(Error::RuleViolation(
            "only open handoffs can be inspected".into(),
        ));
    }
    // Receiver (or proposer) may inspect; others refused.
    if req.inspector_person_id != current.to_person_id
        && req.inspector_person_id != current.from_person_id
    {
        return Err(Error::RuleViolation(
            "only proposer or designated receiver may inspect".into(),
        ));
    }
    let mut next = current.clone();
    if next.status == HandoffStatus::Proposed {
        next.status = HandoffStatus::Inspected;
        next.inspected_at_ms = Some(req.now_ms);
        next.version = bump(current.version)?;
        next.updated_at_ms = req.now_ms;
    }
    Ok(next)
}

pub fn apply_handoff_accept(
    current: &TeamHandoff,
    req: &AcceptHandoffRequest,
) -> Result<TeamHandoff> {
    validate_request_key(&req.request_key)?;
    if current.id != req.handoff_id {
        return Err(Error::NotFound("handoff id mismatch".into()));
    }
    // Idempotent accept ignores the pre-accept expected_version: the stored row
    // has already advanced. A different successor or receiver still conflicts.
    if current.status == HandoffStatus::Accepted {
        let same_request = current.accept_request_key.as_deref() == Some(req.request_key.as_str())
            && current.accepted_successor.as_ref() == Some(&req.successor_execution)
            && current.to_person_id == req.acceptor_person_id;
        if same_request {
            return Ok(current.clone());
        }
        return Err(Error::ClaimConflict(
            "handoff already accepted by another request".into(),
        ));
    }
    require_version(current, req.expected_version)?;
    if !current.status.is_open() {
        return Err(Error::RuleViolation(
            "handoff is not open for accept".into(),
        ));
    }
    if req.acceptor_person_id != current.to_person_id {
        return Err(Error::RuleViolation(
            "only the designated receiver may accept".into(),
        ));
    }
    if !req.context_reprepared {
        return Err(Error::RuleViolation(
            "receiver must re-prepare current context before accept; chat summary is insufficient"
                .into(),
        ));
    }
    if req.unknown_executions_open {
        return Err(Error::RuleViolation(
            "unknown executions must be reconciled before granting successor execution".into(),
        ));
    }
    if !(req.prior_execution_stopped || req.prior_reconciled) {
        return Err(Error::RuleViolation(
            "prior execution must be stopped or its results reconciled before accept".into(),
        ));
    }
    // Late writes under an old fence cannot become the current result.
    match (req.expected_current_fence, req.live_fence) {
        (Some(expected), Some(live)) if expected != live => {
            return Err(Error::ClaimConflict(
                "stale fence: late write under old fence cannot become current handoff result"
                    .into(),
            ));
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err(Error::InvalidInput(
                "expected_current_fence and live_fence must both be set or both omitted".into(),
            ));
        }
        _ => {}
    }
    req.successor_execution.validate()?;
    if req.successor_execution.person_id() != &req.acceptor_person_id {
        return Err(Error::InvalidInput(
            "successor execution must be bound to the accepting person".into(),
        ));
    }
    if let Some(proposed) = &current.proposed_successor {
        // Cross-agent / same-person swap: accepted successor person must match proposal target.
        if proposed.person_id() != req.successor_execution.person_id() {
            return Err(Error::RuleViolation(
                "accepted successor person must match the proposed target person".into(),
            ));
        }
    }
    if let Some(exp) = current.expires_at_ms {
        if req.now_ms >= exp {
            return Err(Error::RuleViolation(
                "handoff proposal expired; use timeout path — original retains responsibility"
                    .into(),
            ));
        }
    }
    let mut next = current.clone();
    next.status = HandoffStatus::Accepted;
    next.accepted_successor = Some(req.successor_execution.clone());
    next.accept_request_key = Some(req.request_key.clone());
    next.terminal_at_ms = Some(req.now_ms);
    next.terminal_reason = Some("accepted".into());
    next.version = bump(current.version)?;
    next.updated_at_ms = req.now_ms;
    Ok(next)
}

pub fn apply_handoff_reject(
    current: &TeamHandoff,
    req: &RejectHandoffRequest,
) -> Result<TeamHandoff> {
    validate_request_key(&req.request_key)?;
    require_version(current, req.expected_version)?;
    if !current.status.is_open() {
        return Err(Error::RuleViolation("handoff is not open".into()));
    }
    if req.by_person_id != current.to_person_id {
        return Err(Error::RuleViolation(
            "only the designated receiver may reject".into(),
        ));
    }
    if req.reason.trim().is_empty() || req.reason.len() > 2048 {
        return Err(Error::InvalidInput("reject reason required".into()));
    }
    terminal(current, HandoffStatus::Rejected, &req.reason, req.now_ms)
}

pub fn apply_handoff_cancel(
    current: &TeamHandoff,
    req: &CancelHandoffRequest,
) -> Result<TeamHandoff> {
    validate_request_key(&req.request_key)?;
    require_version(current, req.expected_version)?;
    if !current.status.is_open() {
        return Err(Error::RuleViolation("handoff is not open".into()));
    }
    if req.by_person_id != current.from_person_id {
        return Err(Error::RuleViolation("only the proposer may cancel".into()));
    }
    if req.reason.trim().is_empty() || req.reason.len() > 2048 {
        return Err(Error::InvalidInput("cancel reason required".into()));
    }
    terminal(current, HandoffStatus::Cancelled, &req.reason, req.now_ms)
}

/// Timeout closes the proposal only. It does **not** stop execution.
pub fn apply_handoff_timeout(
    current: &TeamHandoff,
    req: &TimeoutHandoffRequest,
) -> Result<TeamHandoff> {
    validate_request_key(&req.request_key)?;
    require_version(current, req.expected_version)?;
    if !current.status.is_open() {
        return Err(Error::RuleViolation("handoff is not open".into()));
    }
    let Some(exp) = current.expires_at_ms else {
        return Err(Error::RuleViolation(
            "handoff has no expiry; cannot timeout".into(),
        ));
    };
    if req.now_ms < exp {
        return Err(Error::RuleViolation(
            "handoff expiry not reached; timeout refused".into(),
        ));
    }
    terminal(
        current,
        HandoffStatus::TimedOut,
        "proposal expired; original retains responsibility and recovery; execution not stopped",
        req.now_ms,
    )
}

fn terminal(
    current: &TeamHandoff,
    status: HandoffStatus,
    reason: &str,
    now_ms: i64,
) -> Result<TeamHandoff> {
    let mut next = current.clone();
    next.status = status;
    next.terminal_at_ms = Some(now_ms);
    next.terminal_reason = Some(reason.into());
    next.version = bump(current.version)?;
    next.updated_at_ms = now_ms;
    Ok(next)
}

fn require_version(current: &TeamHandoff, expected: u64) -> Result<()> {
    if current.version != expected {
        return Err(Error::RevisionConflict {
            expected,
            actual: current.version,
        });
    }
    Ok(())
}

fn bump(version: u64) -> Result<u64> {
    version
        .checked_add(1)
        .ok_or_else(|| Error::InvalidInput("handoff version overflow".into()))
}

/// Resolve concurrent accept races: only one effective result.
pub fn resolve_concurrent_accept(first: &TeamHandoff, contender_request_key: &str) -> Result<()> {
    if first.status != HandoffStatus::Accepted {
        return Err(Error::RuleViolation(
            "resolve_concurrent_accept requires an accepted handoff".into(),
        ));
    }
    if first.accept_request_key.as_deref() == Some(contender_request_key) {
        return Ok(());
    }
    Err(Error::ClaimConflict(
        "exactly one concurrent accept is effective; contender discarded".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(s: &str) -> PersonId {
        PersonId::new(s).unwrap()
    }

    fn digest(n: u8) -> String {
        format!("{n:064x}")
    }

    fn package(from: &str) -> HandoffPackage {
        HandoffPackage {
            task_id: "work-a".into(),
            contract_version: "3".into(),
            contract_hash: digest(1),
            current_person_id: person(from),
            current_execution: ExecutionInstance::AgentRun {
                person_id: person(from),
                agent_id: "agent-old".into(),
                binding_id: "bind-old".into(),
            },
            consumed_context_digest: digest(2),
            checkpoint_ids: vec!["cp-1".into()],
            artifact_versions: vec![ArtifactVersionRef {
                artifact_id: "art-1".into(),
                version: "v1".into(),
            }],
            branch_id: Some("branch-1".into()),
            working_directory: Some("crates/awr-core".into()),
            dependency_ids: vec!["dep-1".into()],
            todos: vec!["finish handoff".into()],
            awaiting_replies: vec!["waiting on review".into()],
            unknown_side_effects: vec![],
        }
    }

    fn proposed() -> TeamHandoff {
        apply_handoff_propose(
            "proj-a",
            "work-a",
            &person("alice"),
            &ProposeHandoffRequest {
                request_key: "prop-1".into(),
                handoff_id: "ho-1".into(),
                kind: HandoffKind::Execution,
                package: package("alice"),
                to_person_id: person("bob"),
                proposed_successor: Some(ExecutionInstance::AgentRun {
                    person_id: person("bob"),
                    agent_id: "agent-bob".into(),
                    binding_id: "bind-bob".into(),
                }),
                proposer_execution_id: Some("exec-1".into()),
                proposer_fence: Some(7),
                expires_at_ms: Some(10_000),
                now_ms: 1_000,
            },
        )
        .unwrap()
    }

    #[test]
    fn propose_inspect_accept_cross_person() {
        let mut h = proposed();
        assert_eq!(h.status, HandoffStatus::Proposed);
        let duty = h.duty_at(1_000).unwrap();
        assert_eq!(duty.responsible_person_id, person("alice"));
        assert!(!duty.successor_may_execute);

        h = apply_handoff_inspect(
            &h,
            &InspectHandoffRequest {
                request_key: "ins-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 1,
                inspector_person_id: person("bob"),
                now_ms: 1_500,
            },
        )
        .unwrap();
        assert_eq!(h.status, HandoffStatus::Inspected);

        h = apply_handoff_accept(
            &h,
            &AcceptHandoffRequest {
                request_key: "acc-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 2,
                acceptor_person_id: person("bob"),
                successor_execution: ExecutionInstance::AgentRun {
                    person_id: person("bob"),
                    agent_id: "agent-bob".into(),
                    binding_id: "bind-bob".into(),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: Some(8),
                live_fence: Some(8),
                unknown_executions_open: false,
                now_ms: 2_000,
            },
        )
        .unwrap();
        assert_eq!(h.status, HandoffStatus::Accepted);
        let duty = h.duty_at(2_000).unwrap();
        assert_eq!(duty.responsible_person_id, person("alice")); // execution handoff
        assert_eq!(duty.may_continue_person_id, person("bob"));
        assert!(duty.successor_may_execute);
        assert!(duty.context_requires_refresh);
    }

    #[test]
    fn same_person_agent_swap_keeps_responsibility() {
        let h = apply_handoff_propose(
            "proj-a",
            "work-a",
            &person("alice"),
            &ProposeHandoffRequest {
                request_key: "prop-swap".into(),
                handoff_id: "ho-swap".into(),
                kind: HandoffKind::Execution,
                package: package("alice"),
                to_person_id: person("alice"),
                proposed_successor: Some(ExecutionInstance::AgentRun {
                    person_id: person("alice"),
                    agent_id: "agent-new".into(),
                    binding_id: "bind-new".into(),
                }),
                proposer_execution_id: None,
                proposer_fence: Some(1),
                expires_at_ms: None,
                now_ms: 1,
            },
        )
        .unwrap();
        let accepted = apply_handoff_accept(
            &h,
            &AcceptHandoffRequest {
                request_key: "acc-swap".into(),
                handoff_id: "ho-swap".into(),
                expected_version: 1,
                acceptor_person_id: person("alice"),
                successor_execution: ExecutionInstance::AgentRun {
                    person_id: person("alice"),
                    agent_id: "agent-new".into(),
                    binding_id: "bind-new".into(),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: None,
                live_fence: None,
                unknown_executions_open: false,
                now_ms: 2,
            },
        )
        .unwrap();
        let duty = accepted.duty_at(2).unwrap();
        assert_eq!(duty.responsible_person_id, person("alice"));
        assert_eq!(duty.may_continue_person_id, person("alice"));
    }

    #[test]
    fn responsibility_transfer_does_not_grant_execution() {
        let h = apply_handoff_propose(
            "proj-a",
            "work-a",
            &person("alice"),
            &ProposeHandoffRequest {
                request_key: "prop-own".into(),
                handoff_id: "ho-own".into(),
                kind: HandoffKind::Responsibility,
                package: package("alice"),
                to_person_id: person("bob"),
                proposed_successor: None,
                proposer_execution_id: None,
                proposer_fence: None,
                expires_at_ms: None,
                now_ms: 1,
            },
        )
        .unwrap();
        let accepted = apply_handoff_accept(
            &h,
            &AcceptHandoffRequest {
                request_key: "acc-own".into(),
                handoff_id: "ho-own".into(),
                expected_version: 1,
                acceptor_person_id: person("bob"),
                successor_execution: ExecutionInstance::Person {
                    person_id: person("bob"),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: None,
                live_fence: None,
                unknown_executions_open: false,
                now_ms: 2,
            },
        )
        .unwrap();
        let duty = accepted.duty_at(2).unwrap();
        assert_eq!(duty.responsible_person_id, person("bob"));
        assert!(!duty.successor_may_execute);
        let mut changed = AcceptHandoffRequest {
            request_key: "acc-own".into(),
            handoff_id: "ho-own".into(),
            expected_version: accepted.version,
            acceptor_person_id: person("bob"),
            successor_execution: ExecutionInstance::Person {
                person_id: person("carol"),
            },
            prior_execution_stopped: true,
            prior_reconciled: false,
            context_reprepared: true,
            expected_current_fence: None,
            live_fence: None,
            unknown_executions_open: false,
            now_ms: 3,
        };
        assert!(apply_handoff_accept(&accepted, &changed).is_err());
        changed.successor_execution = ExecutionInstance::Person {
            person_id: person("bob"),
        };
        changed.expected_version = 1;
        assert_eq!(apply_handoff_accept(&accepted, &changed).unwrap(), accepted);
    }

    #[test]
    fn accept_requires_stop_or_reconcile_and_reprepare() {
        let h = proposed();
        assert!(
            apply_handoff_accept(
                &h,
                &AcceptHandoffRequest {
                    request_key: "acc-bad".into(),
                    handoff_id: "ho-1".into(),
                    expected_version: 1,
                    acceptor_person_id: person("bob"),
                    successor_execution: ExecutionInstance::Person {
                        person_id: person("bob"),
                    },
                    prior_execution_stopped: false,
                    prior_reconciled: false,
                    context_reprepared: true,
                    expected_current_fence: Some(1),
                    live_fence: Some(1),
                    unknown_executions_open: false,
                    now_ms: 2,
                },
            )
            .is_err()
        );
        assert!(
            apply_handoff_accept(
                &h,
                &AcceptHandoffRequest {
                    request_key: "acc-bad2".into(),
                    handoff_id: "ho-1".into(),
                    expected_version: authentication_version(&h),
                    acceptor_person_id: person("bob"),
                    successor_execution: ExecutionInstance::Person {
                        person_id: person("bob"),
                    },
                    prior_execution_stopped: true,
                    prior_reconciled: false,
                    context_reprepared: false,
                    expected_current_fence: Some(1),
                    live_fence: Some(1),
                    unknown_executions_open: false,
                    now_ms: 2,
                },
            )
            .is_err()
        );
    }

    fn authentication_version(h: &TeamHandoff) -> u64 {
        h.version
    }

    #[test]
    fn stale_fence_and_unknown_execution_block_accept() {
        let h = proposed();
        assert!(matches!(
            apply_handoff_accept(
                &h,
                &AcceptHandoffRequest {
                    request_key: "acc-fence".into(),
                    handoff_id: "ho-1".into(),
                    expected_version: 1,
                    acceptor_person_id: person("bob"),
                    successor_execution: ExecutionInstance::Person {
                        person_id: person("bob"),
                    },
                    prior_execution_stopped: true,
                    prior_reconciled: false,
                    context_reprepared: true,
                    expected_current_fence: Some(7),
                    live_fence: Some(9),
                    unknown_executions_open: false,
                    now_ms: 2,
                },
            ),
            Err(Error::ClaimConflict(_))
        ));
        assert!(
            apply_handoff_accept(
                &h,
                &AcceptHandoffRequest {
                    request_key: "acc-unk".into(),
                    handoff_id: "ho-1".into(),
                    expected_version: 1,
                    acceptor_person_id: person("bob"),
                    successor_execution: ExecutionInstance::Person {
                        person_id: person("bob"),
                    },
                    prior_execution_stopped: true,
                    prior_reconciled: false,
                    context_reprepared: true,
                    expected_current_fence: Some(9),
                    live_fence: Some(9),
                    unknown_executions_open: true,
                    now_ms: 2,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn timeout_keeps_original_duty_without_stopping() {
        let h = proposed();
        let timed = apply_handoff_timeout(
            &h,
            &TimeoutHandoffRequest {
                request_key: "to-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 1,
                now_ms: 10_000,
            },
        )
        .unwrap();
        assert_eq!(timed.status, HandoffStatus::TimedOut);
        let duty = timed.duty_at(10_000).unwrap();
        assert_eq!(duty.responsible_person_id, person("alice"));
        assert!(!duty.successor_may_execute);
        assert!(duty.note.contains("not stop"));
    }

    #[test]
    fn reject_and_cancel_preserve_original() {
        let h = proposed();
        let rejected = apply_handoff_reject(
            &h,
            &RejectHandoffRequest {
                request_key: "rej-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 1,
                by_person_id: person("bob"),
                reason: "cannot take over".into(),
                now_ms: 3,
            },
        )
        .unwrap();
        assert_eq!(rejected.status, HandoffStatus::Rejected);
        assert_eq!(
            rejected.duty_at(3).unwrap().recovery_duty_person_id,
            person("alice")
        );

        let h2 = proposed();
        let cancelled = apply_handoff_cancel(
            &h2,
            &CancelHandoffRequest {
                request_key: "can-1".into(),
                handoff_id: "ho-1".into(),
                expected_version: 1,
                by_person_id: person("alice"),
                reason: "changed plans".into(),
                now_ms: 4,
            },
        )
        .unwrap();
        assert_eq!(cancelled.status, HandoffStatus::Cancelled);
    }

    #[test]
    fn concurrent_accept_exactly_one() {
        let h = proposed();
        let first = apply_handoff_accept(
            &h,
            &AcceptHandoffRequest {
                request_key: "acc-a".into(),
                handoff_id: "ho-1".into(),
                expected_version: 1,
                acceptor_person_id: person("bob"),
                successor_execution: ExecutionInstance::Person {
                    person_id: person("bob"),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: None,
                live_fence: None,
                unknown_executions_open: false,
                now_ms: 5,
            },
        )
        .unwrap();
        assert!(resolve_concurrent_accept(&first, "acc-a").is_ok());
        assert!(matches!(
            resolve_concurrent_accept(&first, "acc-b"),
            Err(Error::ClaimConflict(_))
        ));
        assert!(matches!(
            apply_handoff_accept(
                &first,
                &AcceptHandoffRequest {
                    request_key: "acc-b".into(),
                    handoff_id: "ho-1".into(),
                    expected_version: first.version,
                    acceptor_person_id: person("bob"),
                    successor_execution: ExecutionInstance::Person {
                        person_id: person("bob"),
                    },
                    prior_execution_stopped: true,
                    prior_reconciled: false,
                    context_reprepared: true,
                    expected_current_fence: None,
                    live_fence: None,
                    unknown_executions_open: false,
                    now_ms: 6,
                },
            ),
            Err(Error::ClaimConflict(_))
        ));
    }

    #[test]
    fn lost_response_same_request_key_is_idempotent_accept() {
        let h = proposed();
        let first = apply_handoff_accept(
            &h,
            &AcceptHandoffRequest {
                request_key: "acc-lost".into(),
                handoff_id: "ho-1".into(),
                expected_version: 1,
                acceptor_person_id: person("bob"),
                successor_execution: ExecutionInstance::Person {
                    person_id: person("bob"),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: None,
                live_fence: None,
                unknown_executions_open: false,
                now_ms: 5,
            },
        )
        .unwrap();
        let replay = apply_handoff_accept(
            &first,
            &AcceptHandoffRequest {
                request_key: "acc-lost".into(),
                handoff_id: "ho-1".into(),
                expected_version: first.version,
                acceptor_person_id: person("bob"),
                successor_execution: ExecutionInstance::Person {
                    person_id: person("bob"),
                },
                prior_execution_stopped: true,
                prior_reconciled: false,
                context_reprepared: true,
                expected_current_fence: None,
                live_fence: None,
                unknown_executions_open: false,
                now_ms: 6,
            },
        )
        .unwrap();
        assert_eq!(replay.version, first.version);
        assert_eq!(replay.status, HandoffStatus::Accepted);
    }
}
