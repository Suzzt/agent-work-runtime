mod support;
use awr_core::*;
use std::collections::BTreeSet;
use support::Fixture;

fn sample_auth(id: &str, project: &str, person: &PersonId) -> AgentAuthorization {
    AgentAuthorization {
        id: id.into(),
        authorizer_person_id: person.clone(),
        responsible_person_id: person.clone(),
        subject_kind: ExecutionSubjectKind::Agent,
        subject_id: "agent-1".into(),
        client_id: "client-1".into(),
        session_id: Some("sess-1".into()),
        model_id: Some("model-a".into()),
        scope: AuthorizationScope::Project {
            project_id: project.into(),
        },
        actions: BTreeSet::from([
            AuthorizedAction::OccupyCollaboratively,
            AuthorizedAction::ClaimCoordination,
            AuthorizedAction::StartWork,
            AuthorizedAction::AcceptResponsibility,
            AuthorizedAction::ManageAuthorization,
        ]),
        expires_at_ms: Some(10_000),
        status: AuthorizationStatus::Active,
        revoked_at_ms: None,
        revoked_by: None,
        verifiable_capabilities: vec![VerifiableCapability::HostDeclared {
            host_id: "host-1".into(),
            capability_id: "shell.exec".into(),
            proof_digest: "abcdef0123456789".into(),
        }],
        self_reported_skill_hints: vec!["rust".into()],
        parent_authorization_id: None,
        maintainer_person_id: None,
        created_at_ms: 1_000,
        binding_id: Some("bind-1".into()),
    }
}

#[test]
fn issue_revoke_list_and_explain_separate_start_work() {
    let mut f = Fixture::new();
    let project = f.project.id;
    let project_id = project.to_string();
    let alice = PersonId::new("alice").unwrap();
    f.store.ensure_person(project, &alice, "Alice").unwrap();

    let auth = sample_auth("auth-1", &project_id, &alice);
    let (stored, receipt) = f
        .store
        .issue_agent_authorization(
            project,
            &IssueAuthorizationRequest {
                request_key: "iss-1".into(),
                authorization: auth.clone(),
            },
        )
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(stored.id, "auth-1");

    let (_, replay) = f
        .store
        .issue_agent_authorization(
            project,
            &IssueAuthorizationRequest {
                request_key: "iss-1".into(),
                authorization: auth.clone(),
            },
        )
        .unwrap();
    assert!(replay.replayed);

    let mut changed = auth.clone();
    changed.actions.remove(&AuthorizedAction::StartWork);
    let changed_issue = f.store.issue_agent_authorization(
        project,
        &IssueAuthorizationRequest {
            request_key: "iss-1".into(),
            authorization: changed,
        },
    );
    assert!(
        changed_issue.is_err(),
        "same request key with a different grant must not replay the old authorization"
    );
    let foreign = sample_auth("auth-foreign", "other-project", &alice);
    assert!(
        f.store
            .issue_agent_authorization(
                project,
                &IssueAuthorizationRequest {
                    request_key: "iss-foreign".into(),
                    authorization: foreign,
                },
            )
            .is_err(),
        "a grant scoped to another project must not be stored here"
    );

    assert_eq!(
        f.store
            .list_agent_authorizations(project, Some(&alice), None, true)
            .unwrap()
            .len(),
        1
    );

    let task = TaskResponsibility::unassigned(&project_id, "work-1");
    let resources = BTreeSet::from(["cpu".into()]);
    let caps = BTreeSet::from(["shell.exec".into()]);
    let executor = ExecutionInstance::AgentRun {
        person_id: alice.clone(),
        agent_id: "agent-1".into(),
        binding_id: "bind-1".into(),
    };
    let explanation = f
        .store
        .explain_claim(&ClaimEvaluationInput {
            project_id: &project_id,
            work_item_id: "work-1",
            task_workstream_id: None,
            candidate_person: &alice,
            authorization: Some(&auth),
            now_ms: 2_000,
            is_project_member: true,
            membership_version: 1,
            assignment_policy: "open",
            assignment_policy_allows: true,
            required_resources: &["cpu".into()],
            available_resource_ids: &resources,
            required_host_capabilities: &["shell.exec".into()],
            verified_host_capabilities: &caps,
            host_id: "host-1",
            dependencies_satisfied: false,
            task: &task,
            requested_executor: &executor,
        })
        .unwrap();
    assert!(explanation.responsibility_accept.allowed);
    assert!(explanation.collaborative_occupancy.allowed);
    assert!(!explanation.start_work_admission.allowed);

    let (revoked, _) = f
        .store
        .revoke_agent_authorization(
            project,
            &RevokeAuthorizationRequest {
                request_key: "rev-1".into(),
                authorization_id: "auth-1".into(),
                revoked_by: alice.clone(),
                revoked_at_ms: 3_000,
                reason: "session ended".into(),
            },
        )
        .unwrap();
    assert!(matches!(revoked.status, AuthorizationStatus::Revoked));
    assert!(
        f.store
            .list_agent_authorizations(project, None, None, true)
            .unwrap()
            .is_empty()
    );
}
