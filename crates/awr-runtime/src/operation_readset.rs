//! Runtime helpers for task-level operation read-sets. Legacy revision CAS paths
//! remain for callers that still supply `expected_revision`.
use awr_core::*;
use awr_store::{SQLITE_COORDINATOR_EPOCH, Store};

/// Observe a work-scoped fact without treating unrelated audit-cursor movement as
/// a semantic conflict. Same-work version and ownership races still conflict.
pub fn append_work_observation(
    store: &mut Store,
    project: Id,
    identity: OperationIdentity,
    draft: EventDraft,
) -> Result<Event> {
    let mut supplied = store.prepare_operation_readset(identity)?;
    // prepare fills trusted versions; keep identity as provided by the caller.
    if supplied.coordinator_epoch != SQLITE_COORDINATOR_EPOCH {
        return Err(Error::InvalidInput(
            "sqlite operation read-set requires the local coordinator epoch".into(),
        ));
    }
    // Re-bind payload digest if the caller left a placeholder.
    if supplied.identity.payload_sha256.chars().all(|c| c == '0') {
        let bytes = serde_json::to_vec(&draft.payload)?;
        use sha2::{Digest, Sha256};
        supplied.identity.payload_sha256 = format!("{:x}", Sha256::digest(&bytes));
    }
    store.append_event_with_readset(project, &supplied, draft)
}

/// Replay classification for adapters that persist the original read-set with a receipt.
pub fn classify_operation_replay_result(
    incoming: &OperationReadSet,
    recorded: Option<&OperationReadSet>,
) -> Result<OperationReplay> {
    Store::classify_stored_operation_replay(incoming, recorded)
}
