//! AWR-TMCP-021: planning suggestions and controlled draft candidates on real PG.
#![cfg(feature = "pg-tests")]
mod common;
#[path = "fixtures/workstream_access.rs"]
mod fixture;

use awr_team::{
    DraftChange, DraftDefinitionState, DraftOpKind, OrdinaryPlanningSelfApprovePolicy, TaskDraft,
};
use awr_team_pg::{DraftCandidateCreate, PgError, SourceStore, SuggestionSubmit};
use fixture::*;
use serde_json::json;

fn draft(id: &str, deps: &[&str], state: DraftDefinitionState) -> TaskDraft {
    TaskDraft {
        work_id: id.into(),
        external_key: id.into(),
        title: format!("Task {id}"),
        goals: vec!["delivery".into()],
        scope_paths: vec!["specs/api.md".into()],
        acceptance: vec!["ok".into()],
        required_dependencies: deps.iter().map(|s| (*s).into()).collect(),
        completion_policy: "independent_review".into(),
        definition_state: state,
        split_from: None,
        split_children: vec![],
    }
}

async fn store_and_roles(
) -> (
    std::sync::MutexGuard<'static, ()>,
    tokio_postgres::Client,
    String,
    SourceStore,
) {
    let (guard, admin, db, _read) = setup().await;
    // Token A is actor=agent. Give three role modes via membership updates in tests.
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='maintainer', membership_version=membership_version+1
             WHERE actor_id='agent';
             UPDATE awr_team.workstream_grants SET can_write=true, grant_version=grant_version+1
             WHERE client_id='cli-a';
             INSERT INTO awr_team.work_items(tenant_id,project_id,id,external_key) VALUES
               ('reader-tenant','reader-project','API-1','API-1'),
               ('reader-tenant','reader-project','CLIENT-1','CLIENT-1')
             ON CONFLICT DO NOTHING;",
        )
        .await
        .unwrap();
    let store = SourceStore::from_config(common::with_app_role(&common::test_config(), &db));
    (guard, admin, db, store)
}

#[tokio::test]
async fn developer_can_suggest_reader_cannot_and_suggestion_is_not_executable() {
    let (_g, admin, _db, store) = store_and_roles().await;
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='developer', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let submit = SuggestionSubmit {
        rationale: "Need shared types dependency".into(),
        affected_work_keys: vec!["CLIENT-1".into()],
        proposed_notes: json!({"add":"SHARED-1"}),
        author_person_id: Some("agent".into()),
    };
    let ok = store
        .submit_planning_suggestion(TENANT, PROJECT, A, &submit)
        .await
        .unwrap();
    assert_eq!(ok["claimable"], false);
    assert_eq!(ok["adds_formal_work"], false);
    assert_eq!(ok["mutates_live_deps"], false);
    assert_eq!(ok["mutates_live_acceptance"], false);
    assert!(ok["suggestion_id"].as_str().unwrap().len() > 10);
    assert_eq!(ok["version"], 1);

    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='reader', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let err = store
        .submit_planning_suggestion(TENANT, PROJECT, A, &submit)
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
}

#[tokio::test]
async fn maintainer_draft_diff_approve_publish_and_edit_invalidates_approval() {
    let (_g, admin, _db, store) = store_and_roles().await;
    // Ensure maintainer (already set in store_and_roles).
    let create = DraftCandidateCreate {
        changes: vec![
            DraftChange {
                op: DraftOpKind::CreateTask,
                before: None,
                after: draft("SHARED-1", &[], DraftDefinitionState::Draft),
            },
            DraftChange {
                op: DraftOpKind::EditFields,
                before: Some(draft("CLIENT-1", &["API-1"], DraftDefinitionState::Enabled)),
                after: draft(
                    "CLIENT-1",
                    &["API-1", "SHARED-1"],
                    DraftDefinitionState::Enabled,
                ),
            },
        ],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy::ordinary_default()),
        author_person_id: Some("agent".into()),
    };
    let created = store
        .create_planning_candidate(TENANT, PROJECT, A, &create)
        .await
        .unwrap();
    let candidate_id = created["candidate_id"].as_str().unwrap().to_string();
    let digest = created["candidate_digest"].as_str().unwrap().to_string();
    assert_eq!(created["state"], "drafting");
    assert!(created["diff"]["field_diffs"].as_array().unwrap().len() >= 1);
    assert_eq!(created["hard_delete_history_allowed"], false);
    assert_eq!(created["forge_completion_via_status_allowed"], false);

    let preview = store
        .preview_planning_candidate(TENANT, PROJECT, A, &candidate_id)
        .await
        .unwrap();
    assert_eq!(preview["diff"]["candidate_digest"], digest);

    let approved = store
        .approve_planning_candidate(TENANT, PROJECT, A, &candidate_id, &digest, Some("agent"))
        .await
        .unwrap();
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["self_approved"], true);
    assert_eq!(approved["independent_review_downgraded"], false);

    let published = store
        .publish_planning_candidate(TENANT, PROJECT, A, &candidate_id, &digest)
        .await
        .unwrap();
    assert_eq!(published["state"], "published");
    assert_eq!(published["source_bytes_written"], false);
    assert_eq!(published["source_writeback_pending"], true);

    // New candidate to exercise edit→stale approval.
    let created2 = store
        .create_planning_candidate(TENANT, PROJECT, A, &create)
        .await
        .unwrap();
    let id2 = created2["candidate_id"].as_str().unwrap().to_string();
    let dig2 = created2["candidate_digest"].as_str().unwrap().to_string();
    store
        .approve_planning_candidate(TENANT, PROJECT, A, &id2, &dig2, Some("agent"))
        .await
        .unwrap();
    let mut edited_after = draft(
        "CLIENT-1",
        &["API-1", "SHARED-1"],
        DraftDefinitionState::Enabled,
    );
    edited_after.title = "Renamed SDK".into();
    let edited = store
        .edit_planning_candidate(
            TENANT,
            PROJECT,
            A,
            &id2,
            vec![
                DraftChange {
                    op: DraftOpKind::CreateTask,
                    before: None,
                    after: draft("SHARED-1", &[], DraftDefinitionState::Draft),
                },
                DraftChange {
                    op: DraftOpKind::EditFields,
                    before: Some(draft("CLIENT-1", &["API-1"], DraftDefinitionState::Enabled)),
                    after: edited_after,
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(edited["prior_approval_cleared"], true);
    assert_ne!(edited["candidate_digest"], dig2);
    let err = store
        .publish_planning_candidate(TENANT, PROJECT, A, &id2, &dig2)
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::CandidateNotApproved | PgError::Protocol(_) | PgError::StaleApproval | PgError::Forbidden),
        "old digest after edit => {err:?}"
    );

    let del = store
        .delete_planning_history(TENANT, PROJECT, A, &id2)
        .await
        .unwrap_err();
    assert!(del.to_string().contains("hard-delete"), "{del}");

    // Developer cannot edit drafts.
    admin
        .batch_execute(
            "UPDATE awr_team.project_memberships SET role='developer', membership_version=membership_version+1
             WHERE actor_id='agent'",
        )
        .await
        .unwrap();
    let err = store
        .create_planning_candidate(TENANT, PROJECT, A, &create)
        .await
        .unwrap_err();
    assert!(matches!(err, PgError::Forbidden), "{err:?}");
}

#[tokio::test]
async fn rejects_cycles_dangling_cross_project_oob_and_policy_downgrade() {
    let (_g, _admin, _db, store) = store_and_roles().await;

    // Cycle between two new tasks.
    let cycle = DraftCandidateCreate {
        changes: vec![
            DraftChange {
                op: DraftOpKind::CreateTask,
                before: None,
                after: draft("X", &["Y"], DraftDefinitionState::Draft),
            },
            DraftChange {
                op: DraftOpKind::CreateTask,
                before: None,
                after: draft("Y", &["X"], DraftDefinitionState::Draft),
            },
        ],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: None,
        author_person_id: Some("agent".into()),
    };
    let err = store
        .create_planning_candidate(TENANT, PROJECT, A, &cycle)
        .await
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("cycle"), "{err}");

    let dangling = DraftCandidateCreate {
        changes: vec![DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: draft("Z", &["MISSING"], DraftDefinitionState::Draft),
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: None,
        author_person_id: Some("agent".into()),
    };
    let err = store
        .create_planning_candidate(TENANT, PROJECT, A, &dangling)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("dangling"), "{err}");

    let mut cross = draft("N1", &[], DraftDefinitionState::Draft);
    cross.goals = vec!["other-tenant-goal".into()];
    let cross_c = DraftCandidateCreate {
        changes: vec![DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: cross,
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: None,
        author_person_id: Some("agent".into()),
    };
    let err = store
        .create_planning_candidate(TENANT, PROJECT, A, &cross_c)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("goal"), "{err}");

    let mut oob = draft("N2", &[], DraftDefinitionState::Draft);
    oob.scope_paths = vec!["../etc/passwd".into()];
    let oob_c = DraftCandidateCreate {
        changes: vec![DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: oob,
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: None,
        author_person_id: Some("agent".into()),
    };
    let err = store
        .create_planning_candidate(TENANT, PROJECT, A, &oob_c)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("out-of-bound"), "{err}");

    let downgrade = DraftCandidateCreate {
        changes: vec![DraftChange {
            op: DraftOpKind::CreateTask,
            before: None,
            after: draft("N3", &[], DraftDefinitionState::Draft),
        }],
        suggestion_ids: vec![],
        allowed_spec_roots: vec!["specs".into()],
        project_goal_keys: vec!["delivery".into()],
        self_approve_policy: Some(OrdinaryPlanningSelfApprovePolicy {
            allow_self_approve_ordinary: true,
            delivery_completion_policy: "author_may_complete".into(),
        }),
        author_person_id: Some("agent".into()),
    };
    let err = store
        .create_planning_candidate(TENANT, PROJECT, A, &downgrade)
        .await
        .unwrap_err();
    assert!(
        matches!(err, PgError::PolicyDowngrade) || err.to_string().contains("downgrad"),
        "{err:?}"
    );
}

#[test]
fn planning_capabilities_surface() {
    let caps = SourceStore::planning_capabilities();
    assert_eq!(caps["suggestion_claimable"], false);
    assert_eq!(caps["approve_publish_separated"], true);
    assert_eq!(caps["source_writeback"], "deferred_to_tmcp_022");
}
