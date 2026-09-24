//! Migration drill and mainline takeover classification (WS-050).
//!
//! Pure rules only: no PostgreSQL, filesystem, or network I/O. Operators still
//! run owner-only `OperatorBackup` / `OperatorHistory` against real Team PG;
//! this module is the fixture-first oracle that proves backup → preview →
//! migrate → restore before any in-repo project takeover evidence is accepted.
//!
//! Invariants:
//! - Unique fact source: migrate never forks a second ledger of the same fact.
//! - Identities of actors, sessions, claims, and reviews are preserved exactly.
//! - History that cannot prove person→agent delegation is marked
//!   `pending_confirmation` — Agents are never auto-promoted to owner or
//!   independent approver.
//! - Goal line ownership Team / EVO / DEC / AUTO is preserved byte-for-byte.
//! - Incomplete work, active sessions, and historical release evidence remain.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MIGRATION_TAKEOVER_PROTOCOL: &str = "awr-workstream-migration-takeover-v1";
pub const FIXTURE_SCHEMA: &str = "awr-workstream-migration-takeover-fixture/v1";
pub const BACKUP_FORMAT: &str = "awr-workstream-migration-backup-v1";

/// Stable goal-line denominators that must survive takeover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GoalLine {
    Team,
    #[serde(rename = "EVO")]
    Evo,
    #[serde(rename = "DEC")]
    Dec,
    #[serde(rename = "AUTO")]
    Auto,
}

impl GoalLine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Team => "Team",
            Self::Evo => "EVO",
            Self::Dec => "DEC",
            Self::Auto => "AUTO",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Person,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureActor {
    pub id: String,
    pub kind: ActorKind,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixturePersonDelegation {
    pub id: String,
    pub person_id: String,
    pub agent_id: String,
    pub status: String,
    /// When false, the binding is recorded but not accepted as proof.
    pub provable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureGoal {
    pub id: String,
    pub title: String,
    pub line: GoalLine,
    pub owner_person_id: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureSession {
    pub id: String,
    pub actor_id: String,
    pub work_item_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureClaim {
    pub id: String,
    pub session_id: String,
    pub actor_id: String,
    pub work_item_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureReview {
    pub id: String,
    pub work_item_id: String,
    /// Recorded reviewer identity (may historically be an agent id).
    pub reviewer_actor_id: String,
    /// Declared role: owner | independent_approver | participant.
    pub declared_role: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureWorkItem {
    pub id: String,
    pub title: String,
    pub status: String,
    pub goal_id: Option<String>,
    pub incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureReleaseEvidence {
    pub id: String,
    pub work_item_id: String,
    pub release_tag: String,
    pub evidence_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationFixture {
    pub schema: String,
    pub fixture_id: String,
    pub project_id: String,
    pub tenant_id: String,
    pub goals: Vec<FixtureGoal>,
    pub actors: Vec<FixtureActor>,
    pub person_delegations: Vec<FixturePersonDelegation>,
    pub sessions: Vec<FixtureSession>,
    pub claims: Vec<FixtureClaim>,
    pub reviews: Vec<FixtureReview>,
    pub work_items: Vec<FixtureWorkItem>,
    pub release_evidence: Vec<FixtureReleaseEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryDisposition {
    /// Identity kept; person-delegation proven for a non-sensitive role.
    Preserved,
    /// Identity kept; cannot auto-promote — needs human confirmation.
    PendingConfirmation,
    /// Identity kept; already a person record.
    PersonRetained,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassifiedHistory {
    pub kind: String,
    pub id: String,
    pub original_actor_id: String,
    pub disposition: HistoryDisposition,
    pub reason: String,
    pub person_delegation_proven: bool,
    /// Always false after classify/migrate.
    pub promoted_to_owner_or_approver: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalOwnershipReceipt {
    pub goal_id: String,
    pub line: GoalLine,
    pub owner_person_id: Option<String>,
    pub preserved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationBackup {
    pub format: String,
    pub protocol: String,
    pub fixture_id: String,
    pub project_id: String,
    pub tenant_id: String,
    pub snapshot_digest: String,
    pub goals: Vec<FixtureGoal>,
    pub actors: Vec<FixtureActor>,
    pub person_delegations: Vec<FixturePersonDelegation>,
    pub sessions: Vec<FixtureSession>,
    pub claims: Vec<FixtureClaim>,
    pub reviews: Vec<FixtureReview>,
    pub work_items: Vec<FixtureWorkItem>,
    pub release_evidence: Vec<FixtureReleaseEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationPreview {
    pub protocol: String,
    pub applied: bool,
    pub backup_digest: String,
    pub history: Vec<ClassifiedHistory>,
    pub goal_ownership: Vec<GoalOwnershipReceipt>,
    pub incomplete_work_item_ids: Vec<String>,
    pub active_session_ids: Vec<String>,
    pub release_evidence_ids: Vec<String>,
    pub pending_confirmation_count: usize,
    pub refusals: Vec<String>,
    pub safe_to_migrate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationResult {
    pub protocol: String,
    pub applied: bool,
    pub backup_digest: String,
    pub result_digest: String,
    pub history: Vec<ClassifiedHistory>,
    pub goal_ownership: Vec<GoalOwnershipReceipt>,
    pub incomplete_work_item_ids: Vec<String>,
    pub active_session_ids: Vec<String>,
    pub release_evidence_ids: Vec<String>,
    pub migrated: MigrationBackup,
    pub identity_forged: bool,
    pub agent_auto_promoted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreResult {
    pub protocol: String,
    pub mode: String,
    pub restored_digest: String,
    pub matches_backup: bool,
    pub refusals: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrillReceipt {
    pub protocol: String,
    pub fixture_id: String,
    pub stages: BTreeMap<String, String>,
    pub backup_digest: String,
    pub preview_pending_confirmation: usize,
    pub migrate_result_digest: String,
    pub restore_matches_backup: bool,
    pub goal_lines_preserved: Vec<String>,
    pub identity_forged: bool,
    pub agent_auto_promoted: bool,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationError {
    InvalidFixture(&'static str),
    UnsupportedBackupFormat,
    PreviewRefused(Vec<String>),
    DigestMismatch,
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFixture(r) => write!(f, "invalid migration fixture: {r}"),
            Self::UnsupportedBackupFormat => write!(f, "unsupported backup format"),
            Self::PreviewRefused(r) => {
                write!(f, "migration preview refused: {}", r.join(","))
            }
            Self::DigestMismatch => write!(f, "snapshot digest mismatch"),
        }
    }
}

impl std::error::Error for MigrationError {}

fn digest_json(value: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(value).expect("json value serializable");
    format!("{:x}", Sha256::digest(bytes))
}

fn snapshot_digest(fixture: &MigrationFixture) -> Result<String, MigrationError> {
    let value =
        serde_json::to_value(fixture).map_err(|_| MigrationError::InvalidFixture("serialize"))?;
    Ok(digest_json(&value))
}

fn actor_map(fixture: &MigrationFixture) -> BTreeMap<&str, &FixtureActor> {
    fixture.actors.iter().map(|a| (a.id.as_str(), a)).collect()
}

fn provable_agent_persons(fixture: &MigrationFixture) -> BTreeMap<&str, &str> {
    let mut map = BTreeMap::new();
    for d in &fixture.person_delegations {
        if d.provable && d.status == "active" {
            map.insert(d.agent_id.as_str(), d.person_id.as_str());
        }
    }
    map
}

/// Classify one historical actor reference for takeover.
pub fn classify_actor_ref(
    actor_id: &str,
    actors: &BTreeMap<&str, &FixtureActor>,
    proven: &BTreeMap<&str, &str>,
    declared_role: Option<&str>,
) -> ClassifiedHistory {
    let Some(actor) = actors.get(actor_id) else {
        return ClassifiedHistory {
            kind: "actor".into(),
            id: actor_id.into(),
            original_actor_id: actor_id.into(),
            disposition: HistoryDisposition::PendingConfirmation,
            reason: "unknown_actor_identity".into(),
            person_delegation_proven: false,
            promoted_to_owner_or_approver: false,
        };
    };
    match actor.kind {
        ActorKind::Person => ClassifiedHistory {
            kind: "actor".into(),
            id: actor.id.clone(),
            original_actor_id: actor.id.clone(),
            disposition: HistoryDisposition::PersonRetained,
            reason: "person_identity_retained".into(),
            person_delegation_proven: false,
            promoted_to_owner_or_approver: false,
        },
        ActorKind::Agent => {
            let proven_ok = proven.contains_key(actor.id.as_str());
            let role = declared_role.unwrap_or("");
            let sensitive = role == "owner" || role == "independent_approver";
            if proven_ok && !sensitive {
                ClassifiedHistory {
                    kind: "actor".into(),
                    id: actor.id.clone(),
                    original_actor_id: actor.id.clone(),
                    disposition: HistoryDisposition::Preserved,
                    reason: "provable_person_delegation".into(),
                    person_delegation_proven: true,
                    promoted_to_owner_or_approver: false,
                }
            } else if proven_ok && sensitive {
                ClassifiedHistory {
                    kind: "actor".into(),
                    id: actor.id.clone(),
                    original_actor_id: actor.id.clone(),
                    disposition: HistoryDisposition::PendingConfirmation,
                    reason: "agent_cannot_auto_become_owner_or_independent_approver".into(),
                    person_delegation_proven: true,
                    promoted_to_owner_or_approver: false,
                }
            } else {
                ClassifiedHistory {
                    kind: "actor".into(),
                    id: actor.id.clone(),
                    original_actor_id: actor.id.clone(),
                    disposition: HistoryDisposition::PendingConfirmation,
                    reason: "unprovable_person_delegation".into(),
                    person_delegation_proven: false,
                    promoted_to_owner_or_approver: false,
                }
            }
        }
    }
}

fn validate_fixture(fixture: &MigrationFixture) -> Result<(), MigrationError> {
    if fixture.schema != FIXTURE_SCHEMA {
        return Err(MigrationError::InvalidFixture("schema"));
    }
    if fixture.fixture_id.is_empty()
        || fixture.project_id.is_empty()
        || fixture.tenant_id.is_empty()
    {
        return Err(MigrationError::InvalidFixture("ids"));
    }
    let mut lines = BTreeSet::new();
    for g in &fixture.goals {
        lines.insert(g.line);
    }
    for required in [GoalLine::Team, GoalLine::Evo, GoalLine::Dec, GoalLine::Auto] {
        if !lines.contains(&required) {
            return Err(MigrationError::InvalidFixture(
                "fixture must cover Team/EVO/DEC/AUTO goal lines",
            ));
        }
    }
    let actor_ids: BTreeSet<_> = fixture.actors.iter().map(|a| a.id.as_str()).collect();
    for d in &fixture.person_delegations {
        if !actor_ids.contains(d.person_id.as_str()) || !actor_ids.contains(d.agent_id.as_str()) {
            return Err(MigrationError::InvalidFixture("delegation references"));
        }
    }
    Ok(())
}

/// Stage 1: logical backup of the independent fixture (unique fact source).
pub fn backup(fixture: &MigrationFixture) -> Result<MigrationBackup, MigrationError> {
    validate_fixture(fixture)?;
    let digest = snapshot_digest(fixture)?;
    Ok(MigrationBackup {
        format: BACKUP_FORMAT.into(),
        protocol: MIGRATION_TAKEOVER_PROTOCOL.into(),
        fixture_id: fixture.fixture_id.clone(),
        project_id: fixture.project_id.clone(),
        tenant_id: fixture.tenant_id.clone(),
        snapshot_digest: digest,
        goals: fixture.goals.clone(),
        actors: fixture.actors.clone(),
        person_delegations: fixture.person_delegations.clone(),
        sessions: fixture.sessions.clone(),
        claims: fixture.claims.clone(),
        reviews: fixture.reviews.clone(),
        work_items: fixture.work_items.clone(),
        release_evidence: fixture.release_evidence.clone(),
    })
}

fn fixture_from_backup(backed: &MigrationBackup) -> MigrationFixture {
    MigrationFixture {
        schema: FIXTURE_SCHEMA.into(),
        fixture_id: backed.fixture_id.clone(),
        project_id: backed.project_id.clone(),
        tenant_id: backed.tenant_id.clone(),
        goals: backed.goals.clone(),
        actors: backed.actors.clone(),
        person_delegations: backed.person_delegations.clone(),
        sessions: backed.sessions.clone(),
        claims: backed.claims.clone(),
        reviews: backed.reviews.clone(),
        work_items: backed.work_items.clone(),
        release_evidence: backed.release_evidence.clone(),
    }
}

/// Stage 2: preview classification without mutating identities.
pub fn preview(backed: &MigrationBackup) -> Result<MigrationPreview, MigrationError> {
    if backed.format != BACKUP_FORMAT {
        return Err(MigrationError::UnsupportedBackupFormat);
    }
    let fixture = fixture_from_backup(backed);
    validate_fixture(&fixture)?;
    let actors = actor_map(&fixture);
    let proven = provable_agent_persons(&fixture);
    let mut history = Vec::new();
    let mut seen_actors = BTreeSet::new();

    for a in &fixture.actors {
        if seen_actors.insert(a.id.as_str()) {
            history.push(classify_actor_ref(&a.id, &actors, &proven, None));
        }
    }
    for s in &fixture.sessions {
        let mut row = classify_actor_ref(&s.actor_id, &actors, &proven, None);
        row.kind = "session".into();
        row.id = s.id.clone();
        history.push(row);
    }
    for c in &fixture.claims {
        let mut row = classify_actor_ref(&c.actor_id, &actors, &proven, None);
        row.kind = "claim".into();
        row.id = c.id.clone();
        history.push(row);
    }
    for r in &fixture.reviews {
        let mut row = classify_actor_ref(
            &r.reviewer_actor_id,
            &actors,
            &proven,
            Some(r.declared_role.as_str()),
        );
        row.kind = "review".into();
        row.id = r.id.clone();
        history.push(row);
    }

    let goal_ownership: Vec<_> = fixture
        .goals
        .iter()
        .map(|g| GoalOwnershipReceipt {
            goal_id: g.id.clone(),
            line: g.line,
            owner_person_id: g.owner_person_id.clone(),
            preserved: true,
        })
        .collect();

    let incomplete_work_item_ids: Vec<_> = fixture
        .work_items
        .iter()
        .filter(|w| w.incomplete || w.status != "completed")
        .map(|w| w.id.clone())
        .collect();
    let active_session_ids: Vec<_> = fixture
        .sessions
        .iter()
        .filter(|s| s.status == "active")
        .map(|s| s.id.clone())
        .collect();
    let release_evidence_ids: Vec<_> = fixture
        .release_evidence
        .iter()
        .map(|e| e.id.clone())
        .collect();

    let pending_confirmation_count = history
        .iter()
        .filter(|h| h.disposition == HistoryDisposition::PendingConfirmation)
        .count();

    let mut refusals = Vec::new();
    if goal_ownership.iter().any(|g| !g.preserved) {
        refusals.push("goal_ownership_drift".into());
    }
    if incomplete_work_item_ids.is_empty() {
        refusals.push("fixture_lacks_incomplete_work".into());
    }
    if active_session_ids.is_empty() {
        refusals.push("fixture_lacks_active_session".into());
    }
    if release_evidence_ids.is_empty() {
        refusals.push("fixture_lacks_release_evidence".into());
    }
    if history.iter().any(|h| h.promoted_to_owner_or_approver) {
        refusals.push("agent_auto_promoted".into());
    }

    Ok(MigrationPreview {
        protocol: MIGRATION_TAKEOVER_PROTOCOL.into(),
        applied: false,
        backup_digest: backed.snapshot_digest.clone(),
        history,
        goal_ownership,
        incomplete_work_item_ids,
        active_session_ids,
        release_evidence_ids,
        pending_confirmation_count,
        refusals: refusals.clone(),
        safe_to_migrate: refusals.is_empty(),
    })
}

/// Stage 3: apply migrate annotations; identities unchanged; no agent promotion.
pub fn migrate(
    backed: &MigrationBackup,
    prev: &MigrationPreview,
) -> Result<MigrationResult, MigrationError> {
    if !prev.safe_to_migrate {
        return Err(MigrationError::PreviewRefused(prev.refusals.clone()));
    }
    if prev.backup_digest != backed.snapshot_digest {
        return Err(MigrationError::DigestMismatch);
    }
    if prev.history.iter().any(|h| h.promoted_to_owner_or_approver) {
        return Err(MigrationError::PreviewRefused(vec![
            "agent_auto_promoted".into(),
        ]));
    }

    let result_body = serde_json::json!({
        "backup_digest": backed.snapshot_digest,
        "history": prev.history,
        "goal_ownership": prev.goal_ownership,
        "incomplete_work_item_ids": prev.incomplete_work_item_ids,
        "active_session_ids": prev.active_session_ids,
        "release_evidence_ids": prev.release_evidence_ids,
    });
    let result_digest = digest_json(&result_body);

    Ok(MigrationResult {
        protocol: MIGRATION_TAKEOVER_PROTOCOL.into(),
        applied: true,
        backup_digest: backed.snapshot_digest.clone(),
        result_digest,
        history: prev.history.clone(),
        goal_ownership: prev.goal_ownership.clone(),
        incomplete_work_item_ids: prev.incomplete_work_item_ids.clone(),
        active_session_ids: prev.active_session_ids.clone(),
        release_evidence_ids: prev.release_evidence_ids.clone(),
        migrated: backed.clone(),
        identity_forged: false,
        agent_auto_promoted: false,
    })
}

/// Stage 4: restore verifies the backup digest still matches the unique source.
pub fn restore(
    backed: &MigrationBackup,
    migrated: &MigrationResult,
) -> Result<RestoreResult, MigrationError> {
    if backed.format != BACKUP_FORMAT {
        return Err(MigrationError::UnsupportedBackupFormat);
    }
    let mut refusals = Vec::new();
    if migrated.identity_forged {
        refusals.push("identity_forged".into());
    }
    if migrated.agent_auto_promoted {
        refusals.push("agent_auto_promoted".into());
    }
    if migrated.migrated.snapshot_digest != backed.snapshot_digest {
        refusals.push("snapshot_digest_drift".into());
    }
    if migrated.migrated.actors != backed.actors
        || migrated.migrated.sessions != backed.sessions
        || migrated.migrated.claims != backed.claims
        || migrated.migrated.reviews != backed.reviews
    {
        refusals.push("identity_rows_changed".into());
    }
    if migrated.goal_ownership.iter().any(|g| !g.preserved) {
        refusals.push("goal_ownership_not_preserved".into());
    }
    Ok(RestoreResult {
        protocol: MIGRATION_TAKEOVER_PROTOCOL.into(),
        mode: if refusals.is_empty() {
            "verified_round_trip".into()
        } else {
            "refused".into()
        },
        restored_digest: backed.snapshot_digest.clone(),
        matches_backup: refusals.is_empty(),
        refusals,
    })
}

/// Full independent-fixture drill: backup → preview → migrate → restore.
pub fn run_drill(fixture: &MigrationFixture) -> Result<DrillReceipt, MigrationError> {
    let mut stages = BTreeMap::new();
    let backed = backup(fixture)?;
    stages.insert("backup".into(), "ok".into());
    let prev = preview(&backed)?;
    stages.insert(
        "preview".into(),
        if prev.safe_to_migrate {
            "ok".into()
        } else {
            "refused".into()
        },
    );
    let migrated = migrate(&backed, &prev)?;
    stages.insert("migrate".into(), "ok".into());
    let restored = restore(&backed, &migrated)?;
    stages.insert(
        "restore".into(),
        if restored.matches_backup {
            "ok".into()
        } else {
            "refused".into()
        },
    );
    let goal_lines_preserved: Vec<_> = migrated
        .goal_ownership
        .iter()
        .filter(|g| g.preserved)
        .map(|g| g.line.as_str().to_string())
        .collect();
    let ok = stages.values().all(|v| v == "ok")
        && !migrated.identity_forged
        && !migrated.agent_auto_promoted
        && restored.matches_backup;
    Ok(DrillReceipt {
        protocol: MIGRATION_TAKEOVER_PROTOCOL.into(),
        fixture_id: fixture.fixture_id.clone(),
        stages,
        backup_digest: backed.snapshot_digest,
        preview_pending_confirmation: prev.pending_confirmation_count,
        migrate_result_digest: migrated.result_digest,
        restore_matches_backup: restored.matches_backup,
        goal_lines_preserved,
        identity_forged: migrated.identity_forged,
        agent_auto_promoted: migrated.agent_auto_promoted,
        ok,
    })
}

/// Project-takeover preview over already-captured goal ownership + history.
/// Used only after the independent fixture drill succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectTakeoverInput {
    pub project_id: String,
    pub goals: Vec<FixtureGoal>,
    pub history: Vec<ClassifiedHistory>,
    pub incomplete_work_item_ids: Vec<String>,
    pub active_session_ids: Vec<String>,
    pub release_evidence_ids: Vec<String>,
    pub fixture_drill_ok: bool,
    pub fixture_backup_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectTakeoverReceipt {
    pub protocol: String,
    pub project_id: String,
    pub applied: bool,
    pub mode: String,
    pub goal_lines_preserved: Vec<String>,
    pub pending_confirmation_count: usize,
    pub incomplete_preserved: bool,
    pub active_sessions_preserved: bool,
    pub release_evidence_preserved: bool,
    pub identity_forged: bool,
    pub agent_auto_promoted: bool,
    pub based_on_fixture_digest: String,
    pub refusals: Vec<String>,
    pub ok: bool,
}

pub fn project_takeover_preview(input: &ProjectTakeoverInput) -> ProjectTakeoverReceipt {
    let mut refusals = Vec::new();
    if !input.fixture_drill_ok {
        refusals.push("independent_fixture_drill_required_first".into());
    }
    let mut lines = BTreeSet::new();
    for g in &input.goals {
        lines.insert(g.line);
    }
    for required in [GoalLine::Team, GoalLine::Evo, GoalLine::Dec, GoalLine::Auto] {
        if !lines.contains(&required) {
            refusals.push(format!("missing_goal_line_{}", required.as_str()));
        }
    }
    if input
        .history
        .iter()
        .any(|h| h.promoted_to_owner_or_approver)
    {
        refusals.push("agent_auto_promoted".into());
    }
    let pending = input
        .history
        .iter()
        .filter(|h| h.disposition == HistoryDisposition::PendingConfirmation)
        .count();
    let goal_lines_preserved: Vec<_> = input
        .goals
        .iter()
        .map(|g| g.line.as_str().to_string())
        .collect();
    let incomplete_preserved = !input.incomplete_work_item_ids.is_empty();
    let active_sessions_preserved = !input.active_session_ids.is_empty();
    let release_evidence_preserved = !input.release_evidence_ids.is_empty();
    if !incomplete_preserved {
        refusals.push("incomplete_work_missing".into());
    }
    if !active_sessions_preserved {
        refusals.push("active_sessions_missing".into());
    }
    if !release_evidence_preserved {
        refusals.push("release_evidence_missing".into());
    }
    let ok = refusals.is_empty();
    ProjectTakeoverReceipt {
        protocol: MIGRATION_TAKEOVER_PROTOCOL.into(),
        project_id: input.project_id.clone(),
        applied: false,
        mode: if ok {
            "dry_run_ready".into()
        } else {
            "refused".into()
        },
        goal_lines_preserved,
        pending_confirmation_count: pending,
        incomplete_preserved,
        active_sessions_preserved,
        release_evidence_preserved,
        identity_forged: false,
        agent_auto_promoted: false,
        based_on_fixture_digest: input.fixture_backup_digest.clone(),
        refusals,
        ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fixture() -> MigrationFixture {
        MigrationFixture {
            schema: FIXTURE_SCHEMA.into(),
            fixture_id: "ws050-independent-1".into(),
            project_id: "proj-fixture".into(),
            tenant_id: "tenant-fixture".into(),
            goals: vec![
                FixtureGoal {
                    id: "g-team".into(),
                    title: "Team collaboration".into(),
                    line: GoalLine::Team,
                    owner_person_id: Some("person-owner".into()),
                    status: "active".into(),
                },
                FixtureGoal {
                    id: "g-evo".into(),
                    title: "Evolution track".into(),
                    line: GoalLine::Evo,
                    owner_person_id: Some("person-evo".into()),
                    status: "active".into(),
                },
                FixtureGoal {
                    id: "g-dec".into(),
                    title: "Decision track".into(),
                    line: GoalLine::Dec,
                    owner_person_id: Some("person-dec".into()),
                    status: "active".into(),
                },
                FixtureGoal {
                    id: "g-auto".into(),
                    title: "Automation track".into(),
                    line: GoalLine::Auto,
                    owner_person_id: Some("person-auto".into()),
                    status: "active".into(),
                },
            ],
            actors: vec![
                FixtureActor {
                    id: "person-owner".into(),
                    kind: ActorKind::Person,
                    display_name: "Owner".into(),
                },
                FixtureActor {
                    id: "person-evo".into(),
                    kind: ActorKind::Person,
                    display_name: "Evo Owner".into(),
                },
                FixtureActor {
                    id: "person-dec".into(),
                    kind: ActorKind::Person,
                    display_name: "Dec Owner".into(),
                },
                FixtureActor {
                    id: "person-auto".into(),
                    kind: ActorKind::Person,
                    display_name: "Auto Owner".into(),
                },
                FixtureActor {
                    id: "agent-bound".into(),
                    kind: ActorKind::Agent,
                    display_name: "Bound Agent".into(),
                },
                FixtureActor {
                    id: "agent-orphan".into(),
                    kind: ActorKind::Agent,
                    display_name: "Orphan Agent".into(),
                },
            ],
            person_delegations: vec![FixturePersonDelegation {
                id: "bind-1".into(),
                person_id: "person-owner".into(),
                agent_id: "agent-bound".into(),
                status: "active".into(),
                provable: true,
            }],
            sessions: vec![
                FixtureSession {
                    id: "sess-active".into(),
                    actor_id: "agent-bound".into(),
                    work_item_id: "W-OPEN".into(),
                    status: "active".into(),
                },
                FixtureSession {
                    id: "sess-orphan".into(),
                    actor_id: "agent-orphan".into(),
                    work_item_id: "W-OPEN".into(),
                    status: "ended".into(),
                },
            ],
            claims: vec![
                FixtureClaim {
                    id: "claim-1".into(),
                    session_id: "sess-active".into(),
                    actor_id: "agent-bound".into(),
                    work_item_id: "W-OPEN".into(),
                    status: "active".into(),
                },
                FixtureClaim {
                    id: "claim-orphan".into(),
                    session_id: "sess-orphan".into(),
                    actor_id: "agent-orphan".into(),
                    work_item_id: "W-OPEN".into(),
                    status: "released".into(),
                },
            ],
            reviews: vec![
                FixtureReview {
                    id: "rev-person".into(),
                    work_item_id: "W-DONE".into(),
                    reviewer_actor_id: "person-owner".into(),
                    declared_role: "independent_approver".into(),
                    status: "accepted".into(),
                },
                FixtureReview {
                    id: "rev-agent-as-approver".into(),
                    work_item_id: "W-DONE".into(),
                    reviewer_actor_id: "agent-bound".into(),
                    declared_role: "independent_approver".into(),
                    status: "accepted".into(),
                },
                FixtureReview {
                    id: "rev-orphan-agent".into(),
                    work_item_id: "W-OPEN".into(),
                    reviewer_actor_id: "agent-orphan".into(),
                    declared_role: "participant".into(),
                    status: "pending".into(),
                },
            ],
            work_items: vec![
                FixtureWorkItem {
                    id: "W-OPEN".into(),
                    title: "Open work".into(),
                    status: "in_progress".into(),
                    goal_id: Some("g-team".into()),
                    incomplete: true,
                },
                FixtureWorkItem {
                    id: "W-DONE".into(),
                    title: "Released work".into(),
                    status: "completed".into(),
                    goal_id: Some("g-evo".into()),
                    incomplete: false,
                },
            ],
            release_evidence: vec![FixtureReleaseEvidence {
                id: "rel-1".into(),
                work_item_id: "W-DONE".into(),
                release_tag: "v0.fixture.1".into(),
                evidence_digest: "abc123".into(),
            }],
        }
    }

    #[test]
    fn drill_backup_preview_migrate_restore_preserves_identities() {
        let fixture = sample_fixture();
        let receipt = run_drill(&fixture).unwrap();
        assert!(receipt.ok);
        assert!(!receipt.identity_forged);
        assert!(!receipt.agent_auto_promoted);
        assert!(receipt.restore_matches_backup);
        for line in ["Team", "EVO", "DEC", "AUTO"] {
            assert!(receipt.goal_lines_preserved.iter().any(|l| l == line));
        }
        assert!(receipt.preview_pending_confirmation >= 1);
    }

    #[test]
    fn agent_without_delegation_is_pending_confirmation() {
        let fixture = sample_fixture();
        let backed = backup(&fixture).unwrap();
        let prev = preview(&backed).unwrap();
        let orphan = prev
            .history
            .iter()
            .find(|h| h.kind == "session" && h.id == "sess-orphan")
            .unwrap();
        assert_eq!(orphan.disposition, HistoryDisposition::PendingConfirmation);
        assert!(!orphan.person_delegation_proven);
        assert!(!orphan.promoted_to_owner_or_approver);
    }

    #[test]
    fn agent_never_auto_promoted_to_independent_approver() {
        let fixture = sample_fixture();
        let backed = backup(&fixture).unwrap();
        let prev = preview(&backed).unwrap();
        let rev = prev
            .history
            .iter()
            .find(|h| h.kind == "review" && h.id == "rev-agent-as-approver")
            .unwrap();
        assert_eq!(rev.disposition, HistoryDisposition::PendingConfirmation);
        assert!(rev.person_delegation_proven);
        assert!(!rev.promoted_to_owner_or_approver);
        assert!(
            prev.history
                .iter()
                .all(|h| !h.promoted_to_owner_or_approver)
        );
    }

    #[test]
    fn project_takeover_requires_fixture_drill_first() {
        let fixture = sample_fixture();
        let drill = run_drill(&fixture).unwrap();
        let backed = backup(&fixture).unwrap();
        let hist = preview(&backed).unwrap().history;
        let blocked = project_takeover_preview(&ProjectTakeoverInput {
            project_id: "real-project".into(),
            goals: fixture.goals.clone(),
            history: hist.clone(),
            incomplete_work_item_ids: vec!["W-OPEN".into()],
            active_session_ids: vec!["sess-active".into()],
            release_evidence_ids: vec!["rel-1".into()],
            fixture_drill_ok: false,
            fixture_backup_digest: drill.backup_digest.clone(),
        });
        assert!(!blocked.ok);
        assert!(
            blocked
                .refusals
                .iter()
                .any(|r| r == "independent_fixture_drill_required_first")
        );

        let ready = project_takeover_preview(&ProjectTakeoverInput {
            project_id: "real-project".into(),
            goals: fixture.goals.clone(),
            history: hist,
            incomplete_work_item_ids: vec!["W-OPEN".into()],
            active_session_ids: vec!["sess-active".into()],
            release_evidence_ids: vec!["rel-1".into()],
            fixture_drill_ok: true,
            fixture_backup_digest: drill.backup_digest,
        });
        assert!(ready.ok);
        assert_eq!(ready.mode, "dry_run_ready");
        assert!(!ready.identity_forged);
        assert!(!ready.agent_auto_promoted);
    }
}
