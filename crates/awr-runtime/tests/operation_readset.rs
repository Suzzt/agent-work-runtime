#[allow(dead_code)]
#[path = "../../awr-store/tests/support/workstreams.rs"]
mod fixture;
use awr_core::*;
use awr_runtime::append_work_observation;
use fixture::Fixture;

#[test]
fn runtime_observation_ignores_unrelated_audit_cursor() {
    let mut f = Fixture::new();
    let before = f.rev();
    f.event(0, None);
    assert_eq!(f.rev(), before + 1);
    let identity = OperationIdentity {
        work: WorkstreamWorkBinding {
            project_id: f.project.to_string(),
            workstream_id: f.scopes[1],
            work_item_id: f.works[1].to_string(),
        },
        subject: "agent".into(),
        request_id: "runtime-1".into(),
        action: "observe.v1".into(),
        payload_sha256: "0".repeat(64),
    };
    let mut draft = EventDraft::new("report.observed", "runtime scoped note");
    draft.work_item_id = Some(f.works[1]);
    let event = append_work_observation(&mut f.store, f.project, identity, draft).unwrap();
    assert_eq!(event.work_item_id, Some(f.works[1]));
    assert_eq!(event.project_revision, before + 2);
}
