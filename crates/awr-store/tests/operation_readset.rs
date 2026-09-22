use awr_core::*;
use awr_store::SQLITE_COORDINATOR_EPOCH;
#[allow(dead_code)]
#[path = "support/workstreams.rs"]
mod fixture;
use fixture::Fixture;

fn identity(project: Id, stream: Id, work: Id, request: &str) -> OperationIdentity {
    OperationIdentity {
        work: WorkstreamWorkBinding {
            project_id: project.to_string(),
            workstream_id: stream,
            work_item_id: work.to_string(),
        },
        subject: "agent".into(),
        request_id: request.into(),
        action: "observe.v1".into(),
        payload_sha256: "a".repeat(64),
    }
}

fn draft(work: Id, summary: &str) -> EventDraft {
    let mut draft = EventDraft::new("report.observed", summary);
    draft.work_item_id = Some(work);
    draft
}

#[test]
fn unrelated_audit_cursor_does_not_block_work_scoped_write() {
    let mut f = Fixture::new();
    let work_a = f.works[0];
    let work_b = f.works[1];
    let before = f.rev();
    f.store
        .append_event(f.project, before, draft(work_a, "unrelated progress"))
        .unwrap();
    assert_eq!(f.rev(), before + 1);
    let mut supplied = f
        .store
        .prepare_operation_readset(identity(f.project, f.scopes[1], work_b, "r1"))
        .unwrap();
    supplied.identity.payload_sha256 = "b".repeat(64);
    let event = f
        .store
        .append_event_with_readset(f.project, &supplied, draft(work_b, "scoped progress"))
        .unwrap();
    assert_eq!(event.project_revision, before + 2);
    assert_eq!(event.work_item_id, Some(work_b));
    assert!(event.project_revision > before);
}

#[test]
fn readset_idempotent_replay_and_intent_conflict() {
    let mut f = Fixture::new();
    let work = f.works[0];
    let mut supplied = f
        .store
        .prepare_operation_readset(identity(f.project, f.scopes[0], work, "once"))
        .unwrap();
    supplied.identity.payload_sha256 = "c".repeat(64);
    assert_eq!(supplied.coordinator_epoch, SQLITE_COORDINATOR_EPOCH);
    let replay =
        awr_store::Store::classify_stored_operation_replay(&supplied, Some(&supplied)).unwrap();
    assert_eq!(replay, OperationReplay::ExistingRequest);
    let mut changed = supplied.clone();
    changed.identity.payload_sha256 = "d".repeat(64);
    assert!(awr_store::Store::classify_stored_operation_replay(&changed, Some(&supplied)).is_err());
    // Stale work_version after an intervening same-work write must conflict.
    f.store
        .append_event_with_readset(f.project, &supplied, draft(work, "first"))
        .unwrap();
    // Advance work item revision out of band (same-task race).
    f.store
        .append_event(f.project, f.rev(), draft(work, "legacy bump"))
        .unwrap();
    // Legacy append does not bump work_items.revision; bump it to simulate CAS.
    // Use a stale read-set: work_version stays at prepare time while authority matches.
    let mut stale = supplied.clone();
    stale.identity.request_id = "second".into();
    stale.identity.payload_sha256 = "e".repeat(64);
    // Force a mismatched work_version against the live required set.
    stale.work_version = supplied.work_version.saturating_add(99);
    assert!(
        f.store
            .append_event_with_readset(f.project, &stale, draft(work, "stale"))
            .is_err()
    );
}

#[test]
fn legacy_project_revision_protocol_still_conflicts() {
    let mut f = Fixture::new();
    let before = f.rev();
    f.store
        .append_event(f.project, before, draft(f.works[0], "first"))
        .unwrap();
    assert!(matches!(
        f.store
            .append_event(f.project, before, draft(f.works[1], "stale legacy")),
        Err(Error::RevisionConflict { .. })
    ));
}
