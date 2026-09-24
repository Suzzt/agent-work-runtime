use awr_core::{EvidenceLevel, Id, WorkstreamWorkBinding, workstream_adoption::*};

fn fixture() -> (DeliveryRequirement, DeliveryFacts) {
    let provider = WorkstreamWorkBinding {
        project_id: "project".into(),
        work_item_id: "upstream".into(),
        workstream_id: Id::from(1),
    };
    let consumer = WorkstreamWorkBinding {
        project_id: "project".into(),
        work_item_id: "downstream".into(),
        workstream_id: Id::from(2),
    };
    let selected = DeliveryVersion {
        completion_receipt: Id::from(3),
        contract_sha256: "a".repeat(64),
        artifact_sha256: "b".repeat(64),
        source_sha: "c".repeat(40),
        environment: "candidate-v1".into(),
        acceptance_round: "round-1".into(),
        export_scope_sha256: "d".repeat(64),
    };
    let requirement = DeliveryRequirement {
        provider: provider.clone(),
        consumer: consumer.clone(),
        selected: selected.clone(),
        policy: DeliveryVersionPolicy::FixedDelivery,
        minimum_level: EvidenceLevel::LocallyVerified,
    };
    let facts = DeliveryFacts {
        provider,
        consumer,
        delivery: Some(selected.clone()),
        current_selection: Some(selected),
        acceptance: DeliveryAcceptance::Verified {
            evidence_id: Id::from(4),
            author: "author".into(),
            reviewer: "independent-reviewer".into(),
            level: EvidenceLevel::LocallyVerified,
            verified_at_ms: 90,
        },
        availability: DeliveryAvailability::Available,
        export_authority: DeliveryExportAuthority::Granted,
        observed_at_ms: 100,
    };
    (requirement, facts)
}
fn expect(
    required: &DeliveryRequirement,
    facts: &DeliveryFacts,
    status: DeliveryStatus,
    reason: DeliveryReason,
) {
    assert_eq!(
        assess_delivery(required, facts).unwrap(),
        DeliveryAssessment { status, reason }
    );
}

#[test]
fn fixed_delivery_survives_new_planning_and_retains_original_proof() {
    let (required, mut facts) = fixture();
    let adopted = adopt_delivery(required, facts.clone(), 100).unwrap();
    let original = adopted.clone();
    facts.observed_at_ms = 200;
    facts.current_selection.as_mut().unwrap().contract_sha256 = "e".repeat(64);
    facts.current_selection.as_mut().unwrap().completion_receipt = Id::from(8);
    assert_eq!(
        adopted.reassess(&facts).unwrap().status,
        DeliveryStatus::Satisfied
    );
    facts.current_selection = None;
    assert_eq!(
        adopted.reassess(&facts).unwrap().status,
        DeliveryStatus::Satisfied
    );
    facts.availability = DeliveryAvailability::Unavailable;
    assert_eq!(
        adopted.reassess(&facts).unwrap().reason,
        DeliveryReason::ArtifactUnavailable
    );
    facts.acceptance = DeliveryAcceptance::Revoked;
    assert_eq!(
        adopted.reassess(&facts).unwrap().status,
        DeliveryStatus::Revoked
    );
    assert_eq!(adopted, original);
    assert_eq!(adopted.adopted_at_ms(), 100);
    assert_eq!(adopted.original_proof().observed_at_ms, 100);
    assert_eq!(
        adopted.requirement().selected.completion_receipt,
        Id::from(3)
    );
}

#[test]
fn current_contract_needs_explicit_current_selection() {
    let (mut required, mut facts) = fixture();
    required.policy = DeliveryVersionPolicy::CurrentContract;
    expect(
        &required,
        &facts,
        DeliveryStatus::Satisfied,
        DeliveryReason::VerifiedDelivery,
    );
    facts.current_selection = None;
    expect(
        &required,
        &facts,
        DeliveryStatus::Unknown,
        DeliveryReason::CurrentSelectionUnknown,
    );
    facts.current_selection = facts.delivery.clone();
    facts.current_selection.as_mut().unwrap().completion_receipt = Id::from(9);
    expect(
        &required,
        &facts,
        DeliveryStatus::Stale,
        DeliveryReason::CurrentSelectionChanged,
    );
    facts.export_authority = DeliveryExportAuthority::Revoked;
    expect(
        &required,
        &facts,
        DeliveryStatus::Revoked,
        DeliveryReason::ExportRevoked,
    );
}

#[test]
fn every_version_dimension_is_exact() {
    let (required, facts) = fixture();
    let mutations: Vec<Box<dyn Fn(&mut DeliveryVersion)>> = vec![
        Box::new(|v| v.completion_receipt = Id::from(99)),
        Box::new(|v| v.contract_sha256 = "f".repeat(64)),
        Box::new(|v| v.artifact_sha256 = "f".repeat(64)),
        Box::new(|v| v.source_sha = "f".repeat(40)),
        Box::new(|v| v.environment = "other-env".into()),
        Box::new(|v| v.acceptance_round = "round-2".into()),
        Box::new(|v| v.export_scope_sha256 = "f".repeat(64)),
    ];
    for mutate in mutations {
        let mut changed = facts.clone();
        mutate(changed.delivery.as_mut().unwrap());
        expect(
            &required,
            &changed,
            DeliveryStatus::Stale,
            DeliveryReason::DeliveryChanged,
        );
    }
}

#[test]
fn source_done_unknown_untrusted_rejected_and_self_review_never_satisfy() {
    let (required, facts) = fixture();
    use DeliveryAcceptance::*;
    let cases = [
        (
            Unknown,
            DeliveryStatus::Unknown,
            DeliveryReason::AcceptanceUnknown,
        ),
        (
            Untrusted,
            DeliveryStatus::Unknown,
            DeliveryReason::UntrustedAcceptance,
        ),
        (
            AuthorDeclaredDone,
            DeliveryStatus::Waiting,
            DeliveryReason::IndependentAcceptanceRequired,
        ),
        (
            Rejected,
            DeliveryStatus::Waiting,
            DeliveryReason::AcceptanceRejected,
        ),
        (
            Revoked,
            DeliveryStatus::Revoked,
            DeliveryReason::AcceptanceRevoked,
        ),
        (
            Verified {
                evidence_id: Id::from(5),
                author: "same".into(),
                reviewer: "same".into(),
                level: EvidenceLevel::Released,
                verified_at_ms: 90,
            },
            DeliveryStatus::Waiting,
            DeliveryReason::IndependentAcceptanceRequired,
        ),
        (
            Verified {
                evidence_id: Id::from(5),
                author: "a".into(),
                reviewer: "b".into(),
                level: EvidenceLevel::Unknown,
                verified_at_ms: 90,
            },
            DeliveryStatus::Unknown,
            DeliveryReason::AcceptanceUnknown,
        ),
        (
            Verified {
                evidence_id: Id::from(5),
                author: "a".into(),
                reviewer: "b".into(),
                level: EvidenceLevel::Implemented,
                verified_at_ms: 90,
            },
            DeliveryStatus::Waiting,
            DeliveryReason::EvidenceLevelInsufficient,
        ),
    ];
    for (acceptance, status, reason) in cases {
        let mut changed = facts.clone();
        changed.acceptance = acceptance;
        expect(&required, &changed, status, reason);
        assert!(matches!(
            adopt_delivery(required.clone(), changed, 100),
            Err(DeliveryError::NotSatisfied(_))
        ));
    }
}

#[test]
fn missing_unknown_unavailable_and_denied_are_distinct() {
    let (required, facts) = fixture();
    let mut changed = facts.clone();
    changed.delivery = None;
    expect(
        &required,
        &changed,
        DeliveryStatus::Waiting,
        DeliveryReason::MissingDelivery,
    );
    for (availability, status, reason) in [
        (
            DeliveryAvailability::Unknown,
            DeliveryStatus::Unknown,
            DeliveryReason::ArtifactAvailabilityUnknown,
        ),
        (
            DeliveryAvailability::Unavailable,
            DeliveryStatus::Stale,
            DeliveryReason::ArtifactUnavailable,
        ),
    ] {
        let mut changed = facts.clone();
        changed.availability = availability;
        expect(&required, &changed, status, reason);
    }
    for (authority, status, reason) in [
        (
            DeliveryExportAuthority::Unknown,
            DeliveryStatus::Unknown,
            DeliveryReason::ExportAuthorityUnknown,
        ),
        (
            DeliveryExportAuthority::Denied,
            DeliveryStatus::Waiting,
            DeliveryReason::ExportDenied,
        ),
        (
            DeliveryExportAuthority::Revoked,
            DeliveryStatus::Revoked,
            DeliveryReason::ExportRevoked,
        ),
    ] {
        let mut changed = facts.clone();
        changed.export_authority = authority;
        expect(&required, &changed, status, reason);
    }
}

#[test]
fn rejects_cross_project_or_forged_ownership_and_same_scope_dependency() {
    let (required, facts) = fixture();
    let mut other = facts.clone();
    other.provider.workstream_id = Id::from(9);
    assert_eq!(
        assess_delivery(&required, &other),
        Err(DeliveryError::BindingMismatch)
    );
    let mut other = facts.clone();
    other.consumer.work_item_id = "other".into();
    assert_eq!(
        assess_delivery(&required, &other),
        Err(DeliveryError::BindingMismatch)
    );
    let mut request = required.clone();
    request.consumer.project_id = "other".into();
    let mut other = facts.clone();
    other.consumer = request.consumer.clone();
    assert_eq!(
        assess_delivery(&request, &other),
        Err(DeliveryError::BindingMismatch)
    );
    let mut request = required.clone();
    request.consumer.workstream_id = request.provider.workstream_id;
    let mut other = facts.clone();
    other.consumer = request.consumer.clone();
    assert_eq!(
        assess_delivery(&request, &other),
        Err(DeliveryError::BindingMismatch)
    );
    let mut request = required;
    request.consumer = request.provider.clone();
    let mut other = facts;
    other.consumer = request.consumer.clone();
    assert_eq!(
        assess_delivery(&request, &other),
        Err(DeliveryError::BindingMismatch)
    );
}

#[test]
fn metadata_and_full_hashes_are_bounded_and_validated() {
    let (required, facts) = fixture();
    for bad in [
        "a".repeat(63),
        "G".repeat(64),
        "a".repeat(65),
        " ".repeat(64),
    ] {
        let mut request = required.clone();
        request.selected.contract_sha256 = bad;
        assert_eq!(
            assess_delivery(&request, &facts),
            Err(DeliveryError::InvalidDefinition)
        );
    }
    for bad in ["abc1234".into(), "z".repeat(40), "a".repeat(41)] {
        let mut request = required.clone();
        request.selected.source_sha = bad;
        assert_eq!(
            assess_delivery(&request, &facts),
            Err(DeliveryError::InvalidDefinition)
        );
    }
    for bad in [" ".into(), "a\n".into(), "a".repeat(4097)] {
        let mut request = required.clone();
        request.selected.environment = bad;
        assert_eq!(
            assess_delivery(&request, &facts),
            Err(DeliveryError::InvalidDefinition)
        );
    }
    let mut request = required.clone();
    request.minimum_level = EvidenceLevel::Unknown;
    assert_eq!(
        assess_delivery(&request, &facts),
        Err(DeliveryError::InvalidDefinition)
    );
    let mut request = required;
    request.provider.workstream_id = Id::from(0);
    assert_eq!(
        assess_delivery(&request, &facts),
        Err(DeliveryError::InvalidDefinition)
    );
}

#[test]
fn snapshot_times_cannot_precede_evidence_or_adoption() {
    let (required, facts) = fixture();
    assert_eq!(
        adopt_delivery(required.clone(), facts.clone(), 101),
        Err(DeliveryError::InvalidTime)
    );
    let adopted = adopt_delivery(required.clone(), facts.clone(), 100).unwrap();
    let mut older = facts.clone();
    older.observed_at_ms = 99;
    assert_eq!(adopted.reassess(&older), Err(DeliveryError::InvalidTime));
    let mut future_evidence = facts;
    if let DeliveryAcceptance::Verified { verified_at_ms, .. } = &mut future_evidence.acceptance {
        *verified_at_ms = 101;
    }
    assert_eq!(
        assess_delivery(&required, &future_evidence),
        Err(DeliveryError::InvalidTime)
    );
}

fn completion_proof(req: &DeliveryRequirement) -> CompletionAcceptanceProof {
    CompletionAcceptanceProof {
        completion_receipt_id: req.selected.completion_receipt,
        work_item_id: req.provider.work_item_id.clone(),
        contract_sha256: req.selected.contract_sha256.clone(),
        artifact_sha256: req.selected.artifact_sha256.clone(),
        independence_kind: "team_independent".into(),
        team_independent_acceptance: true,
        author_person_id: "author".into(),
        reviewer_person_id: "independent-reviewer".into(),
        evidence_id: Id::from(4),
        evidence_level: EvidenceLevel::LocallyVerified,
        verified_at_ms: 90,
    }
}

#[test]
fn completion_self_report_never_unlocks_execution() {
    let (required, _) = fixture();
    let mut proof = completion_proof(&required);
    proof.team_independent_acceptance = false;
    proof.independence_kind = "author_self_report".into();
    assert_eq!(
        acceptance_from_completion_proof(&proof),
        DeliveryAcceptance::AuthorDeclaredDone
    );
    proof.independence_kind = "personal_self_review".into();
    assert_eq!(
        acceptance_from_completion_proof(&proof),
        DeliveryAcceptance::AuthorDeclaredDone
    );
    proof.independence_kind = "ordinary_confirm".into();
    assert_eq!(
        acceptance_from_completion_proof(&proof),
        DeliveryAcceptance::Untrusted
    );
    let mut good = completion_proof(&required);
    good.author_person_id = "same".into();
    good.reviewer_person_id = "same".into();
    assert_eq!(
        acceptance_from_completion_proof(&good),
        DeliveryAcceptance::AuthorDeclaredDone
    );
}

#[test]
fn hard_dependency_binds_works_and_refuses_cross_project() {
    let (required, _) = fixture();
    let dep = validate_hard_dependency_registration(&RegisterHardDependencyRequest {
        request_key: "reg-1".into(),
        dependency_id: "dep-1".into(),
        provider: required.provider.clone(),
        consumer: required.consumer.clone(),
        selected: required.selected.clone(),
        policy: DeliveryVersionPolicy::FixedDelivery,
        minimum_level: EvidenceLevel::LocallyVerified,
        now_ms: 10,
    })
    .unwrap();
    assert_eq!(dep.status, HardDependencyStatus::Active);
    let mut cross = RegisterHardDependencyRequest {
        request_key: "reg-2".into(),
        dependency_id: "dep-2".into(),
        provider: required.provider.clone(),
        consumer: required.consumer.clone(),
        selected: required.selected.clone(),
        policy: DeliveryVersionPolicy::CurrentContract,
        minimum_level: EvidenceLevel::LocallyVerified,
        now_ms: 10,
    };
    cross.consumer.project_id = "other".into();
    assert_eq!(
        validate_hard_dependency_registration(&cross),
        Err(DeliveryError::BindingMismatch)
    );
}

#[test]
fn adopt_credential_requires_trusted_receipt_and_export_grant() {
    let (required, _) = fixture();
    let dependency = validate_hard_dependency_registration(&RegisterHardDependencyRequest {
        request_key: "reg-1".into(),
        dependency_id: "dep-1".into(),
        provider: required.provider.clone(),
        consumer: required.consumer.clone(),
        selected: required.selected.clone(),
        policy: DeliveryVersionPolicy::FixedDelivery,
        minimum_level: EvidenceLevel::LocallyVerified,
        now_ms: 10,
    })
    .unwrap();
    let export = validate_export_grant(&GrantExportAuthorizationRequest {
        request_key: "ex-1".into(),
        authorization_id: "ea-1".into(),
        project_id: required.provider.project_id.clone(),
        provider_work_item_id: required.provider.work_item_id.clone(),
        delivery: required.selected.clone(),
        granted_by: "owner".into(),
        now_ms: 20,
    })
    .unwrap();
    let mut proof = completion_proof(&required);
    proof.independence_kind = "author_self_report".into();
    proof.team_independent_acceptance = false;
    let err = adopt_delivery_credential(&AdoptDeliveryRequest {
        request_key: "ad-1".into(),
        credential_id: "ac-1".into(),
        dependency: dependency.clone(),
        completion: proof,
        export_authorization: export.clone(),
        availability: DeliveryAvailability::Available,
        current_selection: Some(required.selected.clone()),
        now_ms: 100,
    })
    .unwrap_err();
    assert!(matches!(err, DeliveryError::NotSatisfied(_)));
    assert!(!delivery_unlocks_execution(match err {
        DeliveryError::NotSatisfied(a) => a,
        _ => unreachable!(),
    }));

    let credential = adopt_delivery_credential(&AdoptDeliveryRequest {
        request_key: "ad-2".into(),
        credential_id: "ac-2".into(),
        dependency: dependency.clone(),
        completion: completion_proof(&required),
        export_authorization: export,
        availability: DeliveryAvailability::Available,
        current_selection: Some(required.selected.clone()),
        now_ms: 100,
    })
    .unwrap();
    assert_eq!(credential.status, AdoptionCredentialStatus::Active);
    assert!(delivery_unlocks_execution(DeliveryAssessment {
        status: credential.assessment_status,
        reason: credential.assessment_reason,
    }));

    // Fixed delivery survives current-contract drift in reassessment.
    let mut facts = DeliveryFacts {
        provider: required.provider.clone(),
        consumer: required.consumer.clone(),
        delivery: Some(required.selected.clone()),
        current_selection: None,
        acceptance: DeliveryAcceptance::Verified {
            evidence_id: Id::from(4),
            author: "author".into(),
            reviewer: "independent-reviewer".into(),
            level: EvidenceLevel::LocallyVerified,
            verified_at_ms: 90,
        },
        availability: DeliveryAvailability::Available,
        export_authority: DeliveryExportAuthority::Granted,
        observed_at_ms: 200,
    };
    let (assessment, status) = reassess_adoption_credential(&credential, &facts).unwrap();
    assert_eq!(assessment.status, DeliveryStatus::Satisfied);
    assert_eq!(status, AdoptionCredentialStatus::Active);
    facts.availability = DeliveryAvailability::Unavailable;
    let (assessment, status) = reassess_adoption_credential(&credential, &facts).unwrap();
    assert_eq!(assessment.reason, DeliveryReason::ArtifactUnavailable);
    assert_eq!(status, AdoptionCredentialStatus::Stale);
}

#[test]
fn export_authorization_history_is_revocable_and_exact() {
    let (required, _) = fixture();
    let export = validate_export_grant(&GrantExportAuthorizationRequest {
        request_key: "ex-1".into(),
        authorization_id: "ea-1".into(),
        project_id: required.provider.project_id.clone(),
        provider_work_item_id: required.provider.work_item_id.clone(),
        delivery: required.selected.clone(),
        granted_by: "owner".into(),
        now_ms: 20,
    })
    .unwrap();
    assert_eq!(
        export.as_authority(&required.selected),
        DeliveryExportAuthority::Granted
    );
    let mut other = required.selected.clone();
    other.artifact_sha256 = "e".repeat(64);
    assert_eq!(
        export.as_authority(&other),
        DeliveryExportAuthority::Unknown
    );
    let revoked = apply_export_revoke(
        &export,
        &RevokeExportAuthorizationRequest {
            request_key: "rv-1".into(),
            authorization_id: "ea-1".into(),
            reason: "scope narrowed".into(),
            now_ms: 30,
        },
    )
    .unwrap();
    assert_eq!(revoked.status, ExportAuthorizationStatus::Revoked);
    assert_eq!(
        revoked.as_authority(&required.selected),
        DeliveryExportAuthority::Revoked
    );
}
