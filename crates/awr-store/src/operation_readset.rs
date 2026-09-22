//! Task-level operation read-sets for SQLite. Legacy `expected_revision` callers
//! keep using `runtime_transaction`; this path CAS-es business tokens only and
//! treats `project_revision` as an ordered audit cursor.
use crate::{
    Store,
    catalog::revision_at,
    db_error,
    transaction::{insert_event, sqlite_revision},
    workstream::recorded_catalog,
};
use awr_core::*;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

fn map_readset(err: OperationReadSetError) -> Error {
    match err {
        OperationReadSetError::IdempotencyConflict => {
            Error::MutationConflict("operation identity was reused with different intent".into())
        }
        OperationReadSetError::Changed(_)
        | OperationReadSetError::ChangedToken
        | OperationReadSetError::MissingToken
        | OperationReadSetError::UnexpectedToken
        | OperationReadSetError::IdentityMismatch => {
            Error::MutationConflict(format!("operation precondition changed: {err}"))
        }
        other => Error::InvalidInput(other.to_string()),
    }
}

pub const SQLITE_COORDINATOR_EPOCH: &str = "sqlite-local";

fn work_contract_sha256(conn: &Connection, project: Id, work: Id) -> Result<String> {
    let payload: String = conn
        .query_row(
            "SELECT payload_json FROM work_items WHERE project_id=?1 AND id=?2 AND active=1",
            params![project.to_string(), work.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| Error::NotFound(format!("work {work}")))?;
    use sha2::{Digest, Sha256};
    Ok(format!("{:x}", Sha256::digest(payload.as_bytes())))
}

fn load_required_readset(
    conn: &Connection,
    supplied: &OperationReadSet,
) -> Result<RequiredOperationReadSet> {
    let project: Id = supplied
        .identity
        .work
        .project_id
        .parse()
        .map_err(|_| Error::InvalidInput("invalid project id in operation identity".into()))?;
    let work: Id = supplied
        .identity
        .work
        .work_item_id
        .parse()
        .map_err(|_| Error::InvalidInput("invalid work id in operation identity".into()))?;
    let catalog = recorded_catalog(conn, &project.to_string())?;
    let policy_revision: Revision = conn
        .query_row(
            "SELECT revision FROM workstream_catalogs WHERE project_id=?1",
            [project.to_string()],
            |row| revision_at(row, 0),
        )
        .map_err(db_error)?;
    let (stream, ownership_revision): (Id, Revision) = conn
        .query_row(
            "SELECT workstream_id,revision FROM workstream_ownership
            WHERE project_id=?1 AND work_item_id=?2",
            params![project.to_string(), work.to_string()],
            |row| Ok((crate::catalog::id_at(row, 0)?, revision_at(row, 1)?)),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| Error::NotFound("workstream ownership".into()))?;
    if stream != supplied.identity.work.workstream_id {
        return Err(WorkstreamError::BindingMismatch.into());
    }
    let definition = catalog.get(stream)?;
    let work_version: Revision = conn
        .query_row(
            "SELECT revision FROM work_items WHERE project_id=?1 AND id=?2 AND active=1",
            params![project.to_string(), work.to_string()],
            |row| revision_at(row, 0),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| Error::NotFound(format!("work {work}")))?;
    let contract_sha256 = work_contract_sha256(conn, project, work)?;
    let ownership_key = format!("ownership:{work}");
    let mut tokens: Vec<OperationToken> = supplied
        .tokens
        .iter()
        .filter(|t| !(t.kind == OperationTokenKind::Source && t.key == ownership_key))
        .cloned()
        .collect();
    tokens.push(OperationToken {
        kind: OperationTokenKind::Source,
        key: ownership_key,
        version: ownership_revision.to_string(),
    });
    tokens.sort_by(|a, b| (&a.kind, &a.key).cmp(&(&b.kind, &b.key)));
    tokens.dedup_by(|a, b| a.kind == b.kind && a.key == b.key);
    let required = OperationReadSet {
        protocol_version: OPERATION_READSET_VERSION,
        identity: supplied.identity.clone(),
        coordinator_epoch: SQLITE_COORDINATOR_EPOCH.into(),
        policy_revision,
        authority_version: definition.authority_version,
        work_version,
        contract_sha256,
        tokens,
    };
    required.validate().map_err(map_readset)?;
    Ok(RequiredOperationReadSet(required))
}

impl Store {
    /// Load the trusted required read-set for a work-scoped write from the
    /// current SQLite snapshot. Project audit cursors are intentionally absent.
    pub fn required_operation_readset(
        &self,
        supplied: &OperationReadSet,
    ) -> Result<RequiredOperationReadSet> {
        let project: Id =
            supplied.identity.work.project_id.parse().map_err(|_| {
                Error::InvalidInput("invalid project id in operation identity".into())
            })?;
        self.project(project)?;
        load_required_readset(&self.conn, supplied)
    }

    /// Apply a work-scoped mutation under an operation read-set. Unrelated audit
    /// cursor advances do not conflict; legacy callers keep `runtime_transaction`.
    pub(crate) fn runtime_transaction_with_readset<T>(
        &mut self,
        project_id: Id,
        supplied: &OperationReadSet,
        mut draft: EventDraft,
        apply: impl FnOnce(&Transaction<'_>, Revision, &mut EventDraft) -> Result<T>,
    ) -> Result<(T, Event)> {
        let preliminary = load_required_readset(&self.conn, supplied)?;
        validate_operation_readset(supplied, &preliminary).map_err(map_readset)?;
        if draft.event_type.trim().is_empty()
            || !draft.payload.is_object()
            || !["low", "normal", "high", "critical"].contains(&draft.importance.as_str())
        {
            return Err(Error::InvalidInput(
                "event requires a type, object payload and valid importance".into(),
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let live = load_required_readset(&tx, supplied)?;
        validate_operation_readset(supplied, &live).map_err(map_readset)?;
        let actual = tx
            .query_row(
                "SELECT project_revision FROM projects WHERE id=?1",
                [project_id.to_string()],
                |r| revision_at(r, 0),
            )
            .optional()
            .map_err(db_error)?
            .ok_or_else(|| Error::NotFound(format!("project {project_id}")))?;
        let next = actual
            .checked_add(1)
            .ok_or_else(|| Error::InvalidInput("revision overflow".into()))?;
        let next_sql = sqlite_revision(next)?;
        let expected_sql = sqlite_revision(actual)?;
        let result = apply(&tx, next, &mut draft)?;
        crate::events::bind_event(&tx, project_id, &mut draft, false)?;
        let updated = tx
            .execute(
                "UPDATE projects SET project_revision=?1 WHERE id=?2 AND project_revision=?3",
                params![next_sql, project_id.to_string(), expected_sql],
            )
            .map_err(db_error)?;
        if updated != 1 {
            return Err(Error::MutationConflict(
                "transaction changed project revision outside the domain contract".into(),
            ));
        }
        crate::mcp::tag_operation(project_id, &mut draft.payload);
        let event = Event {
            id: Id::new(),
            project_id,
            work_item_id: draft.work_item_id,
            session_id: draft.session_id,
            branch_id: draft.branch_id,
            event_type: draft.event_type,
            importance: draft.importance,
            summary: draft.summary,
            payload: draft.payload,
            project_revision: next,
            created_at: now_millis()?,
        };
        insert_event(&tx, &event)?;
        tx.commit().map_err(db_error)?;
        Ok((result, event))
    }

    /// Append a work-scoped observation under an operation read-set. Compatible
    /// with the legacy project-revision protocol: callers that still pass
    /// `expected_revision` should use `append_event` instead.
    pub fn append_event_with_readset(
        &mut self,
        project_id: Id,
        supplied: &OperationReadSet,
        draft: EventDraft,
    ) -> Result<Event> {
        self.runtime_transaction_with_readset(project_id, supplied, draft, |_, _, _| Ok(()))
            .map(|(_, event)| event)
    }

    /// Build a supplied read-set template from the current trusted snapshot for
    /// the given work. Callers set identity.request_id/action/payload_sha256 and
    /// any extra tokens before commit.
    pub fn prepare_operation_readset(
        &self,
        identity: OperationIdentity,
    ) -> Result<OperationReadSet> {
        let draft = OperationReadSet {
            protocol_version: OPERATION_READSET_VERSION,
            identity,
            coordinator_epoch: SQLITE_COORDINATOR_EPOCH.into(),
            policy_revision: 0,
            authority_version: 1,
            work_version: 0,
            contract_sha256: "0".repeat(64),
            tokens: Vec::new(),
        };
        Ok(self.required_operation_readset(&draft)?.0)
    }

    /// Classify an idempotent repeat against a previously recorded read-set.
    pub fn classify_stored_operation_replay(
        incoming: &OperationReadSet,
        recorded: Option<&OperationReadSet>,
    ) -> Result<OperationReplay> {
        classify_operation_replay(incoming, recorded).map_err(map_readset)
    }
}
