//! Versioned cross-stream delivery dependencies and adoption credentials (WS-030).
//!
//! Pure rules only: no storage, authorization, dispatch or IO. Adapters must
//! authenticate the caller, resolve canonical work ownership and verify
//! receipt/report bytes, reviewer independence and export authority before
//! constructing `DeliveryFacts`. Those deliberately non-Serde facts are NOT wire
//! authorization claims. Persistable credentials (`HardDeliveryDependency`,
//! `ExportAuthorization`, `AdoptionCredential`) carry exact Work+contract+
//! artifact+receipt bindings; author self-reported done never unlocks execution.
//! Fixed-delivery vs current-contract policies are explicit. Cross-project scope
//! is refused. A runtime must recheck a coherent action-specific snapshot
//! atomically with adoption/claim/dispatch/completion and retain historical
//! bindings when validity changes during execution.
use crate::{EvidenceLevel, Id, WorkstreamWorkBinding, is_source_sha, verification_rank};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Exact immutable delivery identity, never a mutable path or a "latest" alias.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryVersion {
    pub completion_receipt: Id,
    pub contract_sha256: String,
    pub artifact_sha256: String,
    pub source_sha: String,
    pub environment: String,
    pub acceptance_round: String,
    /// Hash of the exact approved disclosure scope, not an arbitrary path.
    pub export_scope_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryVersionPolicy {
    FixedDelivery,
    CurrentContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRequirement {
    pub provider: WorkstreamWorkBinding,
    pub consumer: WorkstreamWorkBinding,
    pub selected: DeliveryVersion,
    pub policy: DeliveryVersionPolicy,
    pub minimum_level: EvidenceLevel,
}

/// Provenance is an adapter verdict, never inferred from a source status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryAcceptance {
    Unknown,
    Untrusted,
    AuthorDeclaredDone,
    Rejected,
    Revoked,
    Verified {
        evidence_id: Id,
        author: String,
        reviewer: String,
        level: EvidenceLevel,
        verified_at_ms: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryAvailability {
    Unknown,
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryExportAuthority {
    Unknown,
    Granted,
    Denied,
    Revoked,
}

/// Separate trusted input. Every verdict concerns exactly `delivery` and the
/// two frozen work identities, including its export scope and acceptance round.
/// `current_selection` is the authoritative current contract AND selected receipt;
/// omission is unknown, not permission to resolve an implicit latest version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryFacts {
    pub provider: WorkstreamWorkBinding,
    pub consumer: WorkstreamWorkBinding,
    pub delivery: Option<DeliveryVersion>,
    pub current_selection: Option<DeliveryVersion>,
    pub acceptance: DeliveryAcceptance,
    pub availability: DeliveryAvailability,
    pub export_authority: DeliveryExportAuthority,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Satisfied,
    Waiting,
    Stale,
    Revoked,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryReason {
    VerifiedDelivery,
    MissingDelivery,
    DeliveryChanged,
    CurrentSelectionUnknown,
    CurrentSelectionChanged,
    AcceptanceUnknown,
    UntrustedAcceptance,
    IndependentAcceptanceRequired,
    AcceptanceRejected,
    AcceptanceRevoked,
    EvidenceLevelInsufficient,
    ArtifactAvailabilityUnknown,
    ArtifactUnavailable,
    ExportAuthorityUnknown,
    ExportDenied,
    ExportRevoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryAssessment {
    pub status: DeliveryStatus,
    pub reason: DeliveryReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeliveryError {
    #[error("invalid or unbounded delivery metadata")]
    InvalidDefinition,
    #[error("delivery ownership must bind distinct works and workstreams in one project")]
    BindingMismatch,
    #[error("delivery observation or verification time is invalid")]
    InvalidTime,
    #[error("delivery is not adoptable: {0:?}")]
    NotSatisfied(DeliveryAssessment),
}

/// Created only after evaluation. Getters are immutable; re-evaluation never
/// rewrites the selected version, original proof, or adoption timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdoption {
    requirement: DeliveryRequirement,
    original_proof: DeliveryFacts,
    adopted_at_ms: i64,
}

fn text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}
fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn binding(value: &WorkstreamWorkBinding) -> bool {
    text(&value.project_id) && text(&value.work_item_id) && u128::from(value.workstream_id) != 0
}
fn version(value: &DeliveryVersion) -> bool {
    u128::from(value.completion_receipt) != 0
        && sha256(&value.contract_sha256)
        && sha256(&value.artifact_sha256)
        && sha256(&value.export_scope_sha256)
        && is_source_sha(&value.source_sha)
        && text(&value.environment)
        && text(&value.acceptance_round)
}
fn assessment(status: DeliveryStatus, reason: DeliveryReason) -> DeliveryAssessment {
    DeliveryAssessment { status, reason }
}

/// Deterministic precedence: invalid input, absent/changed receipt, explicit
/// revocation, current-selection drift, acceptance, availability, then authority.
/// A satisfied result concerns this snapshot only and grants no execution right.
pub fn assess_delivery(
    required: &DeliveryRequirement,
    facts: &DeliveryFacts,
) -> Result<DeliveryAssessment, DeliveryError> {
    if !binding(&required.provider)
        || !binding(&required.consumer)
        || !version(&required.selected)
        || verification_rank(required.minimum_level).is_none_or(|rank| rank < 2)
        || facts.delivery.as_ref().is_some_and(|v| !version(v))
        || facts
            .current_selection
            .as_ref()
            .is_some_and(|v| !version(v))
    {
        return Err(DeliveryError::InvalidDefinition);
    }
    if required.provider.project_id != required.consumer.project_id
        || required.provider.work_item_id == required.consumer.work_item_id
        || required.provider.workstream_id == required.consumer.workstream_id
        || required.provider != facts.provider
        || required.consumer != facts.consumer
    {
        return Err(DeliveryError::BindingMismatch);
    }
    if facts.observed_at_ms < 0 {
        return Err(DeliveryError::InvalidTime);
    }
    if let DeliveryAcceptance::Verified {
        evidence_id,
        author,
        reviewer,
        verified_at_ms,
        ..
    } = &facts.acceptance
    {
        if u128::from(*evidence_id) == 0 || !text(author) || !text(reviewer) {
            return Err(DeliveryError::InvalidDefinition);
        }
        if *verified_at_ms < 0 || *verified_at_ms > facts.observed_at_ms {
            return Err(DeliveryError::InvalidTime);
        }
    }
    use DeliveryReason::*;
    use DeliveryStatus::*;
    let Some(delivery) = &facts.delivery else {
        return Ok(assessment(Waiting, MissingDelivery));
    };
    if delivery != &required.selected {
        return Ok(assessment(Stale, DeliveryChanged));
    }
    if facts.acceptance == DeliveryAcceptance::Revoked {
        return Ok(assessment(Revoked, AcceptanceRevoked));
    }
    if facts.export_authority == DeliveryExportAuthority::Revoked {
        return Ok(assessment(Revoked, ExportRevoked));
    }
    if required.policy == DeliveryVersionPolicy::CurrentContract {
        match &facts.current_selection {
            None => return Ok(assessment(Unknown, CurrentSelectionUnknown)),
            Some(current) if current != delivery => {
                return Ok(assessment(Stale, CurrentSelectionChanged));
            }
            _ => (),
        }
    }
    match &facts.acceptance {
        DeliveryAcceptance::Unknown => return Ok(assessment(Unknown, AcceptanceUnknown)),
        DeliveryAcceptance::Untrusted => return Ok(assessment(Unknown, UntrustedAcceptance)),
        DeliveryAcceptance::AuthorDeclaredDone => {
            return Ok(assessment(Waiting, IndependentAcceptanceRequired));
        }
        DeliveryAcceptance::Rejected => return Ok(assessment(Waiting, AcceptanceRejected)),
        DeliveryAcceptance::Verified {
            author,
            reviewer,
            level,
            ..
        } => {
            if author == reviewer {
                return Ok(assessment(Waiting, IndependentAcceptanceRequired));
            }
            let Some(rank) = verification_rank(*level) else {
                return Ok(assessment(Unknown, AcceptanceUnknown));
            };
            if rank < verification_rank(required.minimum_level).unwrap() {
                return Ok(assessment(Waiting, EvidenceLevelInsufficient));
            }
        }
        DeliveryAcceptance::Revoked => unreachable!("handled above"),
    }
    match facts.availability {
        DeliveryAvailability::Unknown => {
            return Ok(assessment(Unknown, ArtifactAvailabilityUnknown));
        }
        DeliveryAvailability::Unavailable => return Ok(assessment(Stale, ArtifactUnavailable)),
        DeliveryAvailability::Available => (),
    }
    Ok(match facts.export_authority {
        DeliveryExportAuthority::Unknown => assessment(Unknown, ExportAuthorityUnknown),
        DeliveryExportAuthority::Denied => assessment(Waiting, ExportDenied),
        DeliveryExportAuthority::Granted => assessment(Satisfied, VerifiedDelivery),
        DeliveryExportAuthority::Revoked => unreachable!("handled above"),
    })
}

pub fn adopt_delivery(
    requirement: DeliveryRequirement,
    facts: DeliveryFacts,
    adopted_at_ms: i64,
) -> Result<DeliveryAdoption, DeliveryError> {
    // The adapter's coherent snapshot must be from the adoption action itself.
    if adopted_at_ms < 0 || adopted_at_ms != facts.observed_at_ms {
        return Err(DeliveryError::InvalidTime);
    }
    let result = assess_delivery(&requirement, &facts)?;
    if result.status != DeliveryStatus::Satisfied {
        return Err(DeliveryError::NotSatisfied(result));
    }
    Ok(DeliveryAdoption {
        requirement,
        original_proof: facts,
        adopted_at_ms,
    })
}

impl DeliveryAdoption {
    pub fn requirement(&self) -> &DeliveryRequirement {
        &self.requirement
    }
    pub fn original_proof(&self) -> &DeliveryFacts {
        &self.original_proof
    }
    pub fn adopted_at_ms(&self) -> i64 {
        self.adopted_at_ms
    }

    /// Recheck for each action from fresh trusted facts, without losing history.
    /// Non-satisfied results require the adapter to prevent new effects/final
    /// acceptance and preserve any prior effects for recovery, not erase them.
    pub fn reassess(&self, facts: &DeliveryFacts) -> Result<DeliveryAssessment, DeliveryError> {
        if facts.observed_at_ms < self.adopted_at_ms {
            return Err(DeliveryError::InvalidTime);
        }
        assess_delivery(&self.requirement, facts)
    }
}

// --- WS-030: versioned hard deps, export authorizations, adoption credentials ---

/// Fields from a WS-018 completion receipt used as trusted acceptance evidence.
/// Adapters copy these from the verified receipt row; this type does not prove
/// provenance by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionAcceptanceProof {
    pub completion_receipt_id: Id,
    pub work_item_id: String,
    pub contract_sha256: String,
    pub artifact_sha256: String,
    pub independence_kind: String,
    pub team_independent_acceptance: bool,
    pub author_person_id: String,
    pub reviewer_person_id: String,
    pub evidence_id: Id,
    pub evidence_level: EvidenceLevel,
    pub verified_at_ms: i64,
}

/// Map a WS-018 completion proof into delivery acceptance.
/// Author self-report, personal self-review, and non-team-independent receipts
/// never become `Verified` and therefore cannot unlock execution.
pub fn acceptance_from_completion_proof(proof: &CompletionAcceptanceProof) -> DeliveryAcceptance {
    if !text(&proof.work_item_id)
        || !sha256(&proof.contract_sha256)
        || !sha256(&proof.artifact_sha256)
        || !text(&proof.independence_kind)
        || !text(&proof.author_person_id)
        || !text(&proof.reviewer_person_id)
        || u128::from(proof.completion_receipt_id) == 0
        || u128::from(proof.evidence_id) == 0
        || proof.verified_at_ms < 0
    {
        return DeliveryAcceptance::Unknown;
    }
    if !proof.team_independent_acceptance || proof.independence_kind != "team_independent" {
        if proof.independence_kind == "personal_self_review"
            || proof.independence_kind == "author_self_report"
            || proof.independence_kind == "caller_asserted"
        {
            return DeliveryAcceptance::AuthorDeclaredDone;
        }
        return DeliveryAcceptance::Untrusted;
    }
    if proof.author_person_id == proof.reviewer_person_id {
        return DeliveryAcceptance::AuthorDeclaredDone;
    }
    DeliveryAcceptance::Verified {
        evidence_id: proof.evidence_id,
        author: proof.author_person_id.clone(),
        reviewer: proof.reviewer_person_id.clone(),
        level: proof.evidence_level,
        verified_at_ms: proof.verified_at_ms,
    }
}

/// Satisfied assessments unlock downstream preparation; everything else does not.
pub fn delivery_unlocks_execution(assessment: DeliveryAssessment) -> bool {
    assessment.status == DeliveryStatus::Satisfied
        && assessment.reason == DeliveryReason::VerifiedDelivery
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardDependencyStatus {
    Active,
    Revoked,
}

/// Cross-stream hard dependency bound to exact Work + contract + artifact + receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardDeliveryDependency {
    pub id: String,
    pub provider: WorkstreamWorkBinding,
    pub consumer: WorkstreamWorkBinding,
    pub selected: DeliveryVersion,
    pub policy: DeliveryVersionPolicy,
    pub minimum_level: EvidenceLevel,
    pub status: HardDependencyStatus,
    pub created_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
}

impl HardDeliveryDependency {
    pub fn requirement(&self) -> DeliveryRequirement {
        DeliveryRequirement {
            provider: self.provider.clone(),
            consumer: self.consumer.clone(),
            selected: self.selected.clone(),
            policy: self.policy,
            minimum_level: self.minimum_level,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterHardDependencyRequest {
    pub request_key: String,
    pub dependency_id: String,
    pub provider: WorkstreamWorkBinding,
    pub consumer: WorkstreamWorkBinding,
    pub selected: DeliveryVersion,
    pub policy: DeliveryVersionPolicy,
    pub minimum_level: EvidenceLevel,
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeHardDependencyRequest {
    pub request_key: String,
    pub dependency_id: String,
    pub expected_status: HardDependencyStatus,
    pub now_ms: i64,
}

pub fn validate_hard_dependency_registration(
    req: &RegisterHardDependencyRequest,
) -> Result<HardDeliveryDependency, DeliveryError> {
    if !text(&req.request_key)
        || !text(&req.dependency_id)
        || req.dependency_id.len() > 128
        || req.request_key.len() > 128
        || req.now_ms < 0
        || !binding(&req.provider)
        || !binding(&req.consumer)
        || !version(&req.selected)
        || verification_rank(req.minimum_level).is_none_or(|rank| rank < 2)
    {
        return Err(DeliveryError::InvalidDefinition);
    }
    if req.provider.project_id != req.consumer.project_id
        || req.provider.work_item_id == req.consumer.work_item_id
        || req.provider.workstream_id == req.consumer.workstream_id
    {
        return Err(DeliveryError::BindingMismatch);
    }
    Ok(HardDeliveryDependency {
        id: req.dependency_id.clone(),
        provider: req.provider.clone(),
        consumer: req.consumer.clone(),
        selected: req.selected.clone(),
        policy: req.policy,
        minimum_level: req.minimum_level,
        status: HardDependencyStatus::Active,
        created_at_ms: req.now_ms,
        revoked_at_ms: None,
    })
}

pub fn apply_hard_dependency_revoke(
    current: &HardDeliveryDependency,
    req: &RevokeHardDependencyRequest,
) -> Result<HardDeliveryDependency, DeliveryError> {
    if !text(&req.request_key)
        || req.request_key.len() > 128
        || req.dependency_id != current.id
        || req.now_ms < 0
        || req.now_ms < current.created_at_ms
    {
        return Err(DeliveryError::InvalidDefinition);
    }
    if current.status != req.expected_status {
        return Err(DeliveryError::InvalidDefinition);
    }
    if current.status == HardDependencyStatus::Revoked {
        return Ok(current.clone());
    }
    Ok(HardDeliveryDependency {
        status: HardDependencyStatus::Revoked,
        revoked_at_ms: Some(req.now_ms),
        ..current.clone()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportAuthorizationStatus {
    Granted,
    Denied,
    Revoked,
}

/// Auditable export authorization for a concrete delivery version + scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportAuthorization {
    pub id: String,
    pub project_id: String,
    pub provider_work_item_id: String,
    pub delivery: DeliveryVersion,
    pub status: ExportAuthorizationStatus,
    pub granted_by: String,
    pub created_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
    pub revoke_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantExportAuthorizationRequest {
    pub request_key: String,
    pub authorization_id: String,
    pub project_id: String,
    pub provider_work_item_id: String,
    pub delivery: DeliveryVersion,
    pub granted_by: String,
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeExportAuthorizationRequest {
    pub request_key: String,
    pub authorization_id: String,
    pub reason: String,
    pub now_ms: i64,
}

pub fn validate_export_grant(
    req: &GrantExportAuthorizationRequest,
) -> Result<ExportAuthorization, DeliveryError> {
    if !text(&req.request_key)
        || !text(&req.authorization_id)
        || !text(&req.project_id)
        || !text(&req.provider_work_item_id)
        || !text(&req.granted_by)
        || !version(&req.delivery)
        || req.now_ms < 0
        || req.request_key.len() > 128
        || req.authorization_id.len() > 128
    {
        return Err(DeliveryError::InvalidDefinition);
    }
    Ok(ExportAuthorization {
        id: req.authorization_id.clone(),
        project_id: req.project_id.clone(),
        provider_work_item_id: req.provider_work_item_id.clone(),
        delivery: req.delivery.clone(),
        status: ExportAuthorizationStatus::Granted,
        granted_by: req.granted_by.clone(),
        created_at_ms: req.now_ms,
        revoked_at_ms: None,
        revoke_reason: None,
    })
}

pub fn apply_export_revoke(
    current: &ExportAuthorization,
    req: &RevokeExportAuthorizationRequest,
) -> Result<ExportAuthorization, DeliveryError> {
    if !text(&req.request_key)
        || !text(&req.reason)
        || req.authorization_id != current.id
        || req.now_ms < 0
        || req.now_ms < current.created_at_ms
        || req.request_key.len() > 128
    {
        return Err(DeliveryError::InvalidDefinition);
    }
    if current.status == ExportAuthorizationStatus::Revoked {
        return Ok(current.clone());
    }
    Ok(ExportAuthorization {
        status: ExportAuthorizationStatus::Revoked,
        revoked_at_ms: Some(req.now_ms),
        revoke_reason: Some(req.reason.clone()),
        ..current.clone()
    })
}

impl ExportAuthorization {
    pub fn as_authority(&self, delivery: &DeliveryVersion) -> DeliveryExportAuthority {
        if &self.delivery != delivery {
            return DeliveryExportAuthority::Unknown;
        }
        match self.status {
            ExportAuthorizationStatus::Granted => DeliveryExportAuthority::Granted,
            ExportAuthorizationStatus::Denied => DeliveryExportAuthority::Denied,
            ExportAuthorizationStatus::Revoked => DeliveryExportAuthority::Revoked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptionCredentialStatus {
    Active,
    Stale,
    Revoked,
}

/// Historical adoption credential: retains original selected version and proof
/// references so fixed-delivery consumers survive unrelated upstream replanning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptionCredential {
    pub id: String,
    pub dependency_id: String,
    pub requirement: DeliveryRequirement,
    pub completion_receipt_id: Id,
    pub export_authorization_id: String,
    pub acceptance_evidence_id: Id,
    pub acceptance_author: String,
    pub acceptance_reviewer: String,
    pub acceptance_level: EvidenceLevel,
    pub acceptance_verified_at_ms: i64,
    pub adopted_at_ms: i64,
    pub status: AdoptionCredentialStatus,
    pub assessment_status: DeliveryStatus,
    pub assessment_reason: DeliveryReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptDeliveryRequest {
    pub request_key: String,
    pub credential_id: String,
    pub dependency: HardDeliveryDependency,
    pub completion: CompletionAcceptanceProof,
    pub export_authorization: ExportAuthorization,
    pub availability: DeliveryAvailability,
    pub current_selection: Option<DeliveryVersion>,
    pub now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryCredentialReceipt {
    pub request_key: String,
    pub subject_id: String,
    pub op: String,
    pub event_id: Id,
    pub replayed: bool,
}

/// Build trusted facts and adopt. Self-reported completion proofs fail closed.
pub fn adopt_delivery_credential(
    req: &AdoptDeliveryRequest,
) -> Result<AdoptionCredential, DeliveryError> {
    if !text(&req.request_key)
        || !text(&req.credential_id)
        || req.request_key.len() > 128
        || req.credential_id.len() > 128
        || req.now_ms < 0
        || req.dependency.status != HardDependencyStatus::Active
    {
        return Err(DeliveryError::InvalidDefinition);
    }
    if req.completion.work_item_id != req.dependency.provider.work_item_id
        || req.completion.contract_sha256 != req.dependency.selected.contract_sha256
        || req.completion.artifact_sha256 != req.dependency.selected.artifact_sha256
        || req.completion.completion_receipt_id != req.dependency.selected.completion_receipt
    {
        return Err(DeliveryError::BindingMismatch);
    }
    if req.export_authorization.project_id != req.dependency.provider.project_id
        || req.export_authorization.provider_work_item_id != req.dependency.provider.work_item_id
        || req.export_authorization.delivery != req.dependency.selected
    {
        return Err(DeliveryError::BindingMismatch);
    }
    let acceptance = acceptance_from_completion_proof(&req.completion);
    let facts = DeliveryFacts {
        provider: req.dependency.provider.clone(),
        consumer: req.dependency.consumer.clone(),
        delivery: Some(req.dependency.selected.clone()),
        current_selection: req.current_selection.clone(),
        acceptance,
        availability: req.availability,
        export_authority: req
            .export_authorization
            .as_authority(&req.dependency.selected),
        observed_at_ms: req.now_ms,
    };
    let adopted = adopt_delivery(req.dependency.requirement(), facts, req.now_ms)?;
    let assessment = DeliveryAssessment {
        status: DeliveryStatus::Satisfied,
        reason: DeliveryReason::VerifiedDelivery,
    };
    if !delivery_unlocks_execution(assessment) {
        return Err(DeliveryError::NotSatisfied(assessment));
    }
    let DeliveryAcceptance::Verified {
        evidence_id,
        author,
        reviewer,
        level,
        verified_at_ms,
    } = adopted.original_proof().acceptance.clone()
    else {
        return Err(DeliveryError::NotSatisfied(DeliveryAssessment {
            status: DeliveryStatus::Waiting,
            reason: DeliveryReason::IndependentAcceptanceRequired,
        }));
    };
    Ok(AdoptionCredential {
        id: req.credential_id.clone(),
        dependency_id: req.dependency.id.clone(),
        requirement: adopted.requirement().clone(),
        completion_receipt_id: req.completion.completion_receipt_id,
        export_authorization_id: req.export_authorization.id.clone(),
        acceptance_evidence_id: evidence_id,
        acceptance_author: author,
        acceptance_reviewer: reviewer,
        acceptance_level: level,
        acceptance_verified_at_ms: verified_at_ms,
        adopted_at_ms: adopted.adopted_at_ms(),
        status: AdoptionCredentialStatus::Active,
        assessment_status: assessment.status,
        assessment_reason: assessment.reason,
    })
}

/// Reassess a stored credential against fresh trusted facts without rewriting history.
pub fn reassess_adoption_credential(
    credential: &AdoptionCredential,
    facts: &DeliveryFacts,
) -> Result<(DeliveryAssessment, AdoptionCredentialStatus), DeliveryError> {
    if facts.observed_at_ms < credential.adopted_at_ms {
        return Err(DeliveryError::InvalidTime);
    }
    let assessment = assess_delivery(&credential.requirement, facts)?;
    let status = match assessment.status {
        DeliveryStatus::Satisfied => AdoptionCredentialStatus::Active,
        DeliveryStatus::Revoked => AdoptionCredentialStatus::Revoked,
        DeliveryStatus::Stale => AdoptionCredentialStatus::Stale,
        DeliveryStatus::Waiting | DeliveryStatus::Unknown => AdoptionCredentialStatus::Stale,
    };
    Ok((assessment, status))
}
