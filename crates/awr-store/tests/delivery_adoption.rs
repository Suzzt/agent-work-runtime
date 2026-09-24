mod support;
use awr_core::*;
use support::Fixture;

fn digest(n: u8) -> String {
    format!("{n:064x}")
}

fn source_sha(n: u8) -> String {
    format!("{n:040x}")
}

fn fixtures(
    project: &str,
) -> (
    DeliveryRequirement,
    CompletionAcceptanceProof,
    GrantExportAuthorizationRequest,
) {
    let provider = WorkstreamWorkBinding {
        project_id: project.into(),
        work_item_id: "upstream".into(),
        workstream_id: Id::from(1),
    };
    let consumer = WorkstreamWorkBinding {
        project_id: project.into(),
        work_item_id: "downstream".into(),
        workstream_id: Id::from(2),
    };
    let selected = DeliveryVersion {
        completion_receipt: Id::from(3),
        contract_sha256: digest(0xa),
        artifact_sha256: digest(0xb),
        source_sha: source_sha(0xc),
        environment: "candidate-v1".into(),
        acceptance_round: "round-1".into(),
        export_scope_sha256: digest(0xd),
    };
    let requirement = DeliveryRequirement {
        provider: provider.clone(),
        consumer,
        selected: selected.clone(),
        policy: DeliveryVersionPolicy::FixedDelivery,
        minimum_level: EvidenceLevel::LocallyVerified,
    };
    let proof = CompletionAcceptanceProof {
        completion_receipt_id: selected.completion_receipt,
        work_item_id: provider.work_item_id.clone(),
        contract_sha256: selected.contract_sha256.clone(),
        artifact_sha256: selected.artifact_sha256.clone(),
        independence_kind: "team_independent".into(),
        team_independent_acceptance: true,
        author_person_id: "author".into(),
        reviewer_person_id: "reviewer".into(),
        evidence_id: Id::from(4),
        evidence_level: EvidenceLevel::LocallyVerified,
        verified_at_ms: 90,
    };
    let export_req = GrantExportAuthorizationRequest {
        request_key: "ex-1".into(),
        authorization_id: "ea-1".into(),
        project_id: project.into(),
        provider_work_item_id: provider.work_item_id,
        delivery: selected,
        granted_by: "owner".into(),
        now_ms: 20,
    };
    (requirement, proof, export_req)
}

#[test]
fn sqlite_register_adopt_roundtrip_and_replay() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let project_s = project.to_string();
    let (req, proof, export_req) = fixtures(&project_s);

    let (dep, receipt) = f
        .store
        .register_hard_delivery_dependency(
            project,
            &RegisterHardDependencyRequest {
                request_key: "reg-1".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .unwrap();
    assert!(!receipt.replayed);
    let (_, replay) = f
        .store
        .register_hard_delivery_dependency(
            project,
            &RegisterHardDependencyRequest {
                request_key: "reg-1".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .unwrap();
    assert!(replay.replayed);

    let (export, _) = f
        .store
        .grant_export_authorization(project, &export_req)
        .unwrap();

    let mut self_report = proof.clone();
    self_report.team_independent_acceptance = false;
    self_report.independence_kind = "author_self_report".into();
    assert!(
        f.store
            .adopt_delivery_credential(
                project,
                &AdoptDeliveryRequest {
                    request_key: "ad-bad".into(),
                    credential_id: "ac-bad".into(),
                    dependency: dep.clone(),
                    completion: self_report,
                    export_authorization: export.clone(),
                    availability: DeliveryAvailability::Available,
                    current_selection: Some(req.selected.clone()),
                    now_ms: 100,
                },
            )
            .is_err()
    );

    let (cred, adopt_receipt) = f
        .store
        .adopt_delivery_credential(
            project,
            &AdoptDeliveryRequest {
                request_key: "ad-1".into(),
                credential_id: "ac-1".into(),
                dependency: dep,
                completion: proof,
                export_authorization: export,
                availability: DeliveryAvailability::Available,
                current_selection: Some(req.selected.clone()),
                now_ms: 100,
            },
        )
        .unwrap();
    assert_eq!(cred.status, AdoptionCredentialStatus::Active);
    assert!(!adopt_receipt.replayed);
    let loaded = f
        .store
        .get_adoption_credential(project, "ac-1")
        .unwrap()
        .unwrap();
    assert_eq!(loaded, cred);
}

#[test]
fn sqlite_refuses_cross_project_dependency() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let (mut req, _, _) = fixtures(&project.to_string());
    req.consumer.project_id = "other-project".into();
    let err = f
        .store
        .register_hard_delivery_dependency(
            project,
            &RegisterHardDependencyRequest {
                request_key: "reg-x".into(),
                dependency_id: "dep-x".into(),
                provider: req.provider,
                consumer: req.consumer,
                selected: req.selected,
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[test]
fn sqlite_export_revoke_is_auditable() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let (_, _, export_req) = fixtures(&project.to_string());
    let (export, _) = f
        .store
        .grant_export_authorization(project, &export_req)
        .unwrap();
    let (revoked, _) = f
        .store
        .revoke_export_authorization(
            project,
            &RevokeExportAuthorizationRequest {
                request_key: "rv-1".into(),
                authorization_id: export.id.clone(),
                reason: "narrowed scope".into(),
                now_ms: 40,
            },
        )
        .unwrap();
    assert_eq!(revoked.status, ExportAuthorizationStatus::Revoked);
    assert_eq!(revoked.revoke_reason.as_deref(), Some("narrowed scope"));
    let loaded = f
        .store
        .get_export_authorization(project, &export.id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded, revoked);
}

#[test]
fn sqlite_refuses_reused_dependency_id() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let (req, _, _) = fixtures(&project.to_string());
    f.store
        .register_hard_delivery_dependency(
            project,
            &RegisterHardDependencyRequest {
                request_key: "reg-1".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider.clone(),
                consumer: req.consumer.clone(),
                selected: req.selected.clone(),
                policy: req.policy,
                minimum_level: req.minimum_level,
                now_ms: 10,
            },
        )
        .unwrap();
    let err = f
        .store
        .register_hard_delivery_dependency(
            project,
            &RegisterHardDependencyRequest {
                request_key: "reg-2".into(),
                dependency_id: "dep-1".into(),
                provider: req.provider,
                consumer: req.consumer,
                selected: req.selected,
                policy: DeliveryVersionPolicy::CurrentContract,
                minimum_level: req.minimum_level,
                now_ms: 11,
            },
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
    let kept = f
        .store
        .get_hard_delivery_dependency(project, "dep-1")
        .unwrap()
        .unwrap();
    assert_eq!(kept.policy, DeliveryVersionPolicy::FixedDelivery);
}
