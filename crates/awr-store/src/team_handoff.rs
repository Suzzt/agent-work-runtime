//! SQLite persistence for confirmed Team handoffs (WS-017).
use crate::{Store, db_error};
use awr_core::{
    AcceptHandoffRequest, CancelHandoffRequest, Error, HandoffReceipt, HandoffStatus, Id,
    InspectHandoffRequest, PersonId, ProposeHandoffRequest, RejectHandoffRequest, Result,
    TeamHandoff, TimeoutHandoffRequest, apply_handoff_accept, apply_handoff_cancel,
    apply_handoff_inspect, apply_handoff_propose, apply_handoff_reject, apply_handoff_timeout,
};
use rusqlite::{OptionalExtension, params};

impl Store {
    pub fn get_team_handoff(&self, project: Id, handoff_id: &str) -> Result<Option<TeamHandoff>> {
        load(&self.conn, &project.to_string(), handoff_id)
    }

    pub fn propose_team_handoff(
        &mut self,
        project: Id,
        work_item_id: &str,
        from_person: &PersonId,
        req: &ProposeHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        let project_s = project.to_string();
        if let Some(receipt) = load_receipt(&self.conn, &project_s, &req.request_key, "propose")? {
            let h = load(&self.conn, &project_s, &receipt.handoff_id)?
                .ok_or_else(|| Error::Storage("handoff missing for receipt".into()))?;
            return Ok((h, receipt));
        }
        let handoff = apply_handoff_propose(&project_s, work_item_id, from_person, req)?;
        persist(&self.conn, &project_s, &handoff)?;
        let receipt = record(
            &self.conn,
            &project_s,
            &handoff,
            &req.request_key,
            "propose",
        )?;
        Ok((handoff, receipt))
    }

    pub fn inspect_team_handoff(
        &mut self,
        project: Id,
        req: &InspectHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.mutate_handoff(project, &req.request_key, "inspect", &req.handoff_id, |h| {
            apply_handoff_inspect(h, req)
        })
    }

    pub fn accept_team_handoff(
        &mut self,
        project: Id,
        req: &AcceptHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.mutate_handoff(project, &req.request_key, "accept", &req.handoff_id, |h| {
            apply_handoff_accept(h, req)
        })
    }

    pub fn reject_team_handoff(
        &mut self,
        project: Id,
        req: &RejectHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.mutate_handoff(project, &req.request_key, "reject", &req.handoff_id, |h| {
            apply_handoff_reject(h, req)
        })
    }

    pub fn cancel_team_handoff(
        &mut self,
        project: Id,
        req: &CancelHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.mutate_handoff(project, &req.request_key, "cancel", &req.handoff_id, |h| {
            apply_handoff_cancel(h, req)
        })
    }

    pub fn timeout_team_handoff(
        &mut self,
        project: Id,
        req: &TimeoutHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.mutate_handoff(project, &req.request_key, "timeout", &req.handoff_id, |h| {
            apply_handoff_timeout(h, req)
        })
    }

    fn mutate_handoff<F>(
        &mut self,
        project: Id,
        request_key: &str,
        op: &str,
        handoff_id: &str,
        f: F,
    ) -> Result<(TeamHandoff, HandoffReceipt)>
    where
        F: FnOnce(&TeamHandoff) -> Result<TeamHandoff>,
    {
        let project_s = project.to_string();
        if let Some(receipt) = load_receipt(&self.conn, &project_s, request_key, op)? {
            if receipt.handoff_id != handoff_id {
                return Err(Error::InvalidInput(
                    "request key was already used for a different handoff".into(),
                ));
            }
            let h = load(&self.conn, &project_s, &receipt.handoff_id)?
                .ok_or_else(|| Error::Storage("handoff missing for receipt".into()))?;
            if op == "accept" {
                let again = f(&h)?;
                if again != h {
                    return Err(Error::InvalidInput(
                        "request key replay does not match the stored handoff".into(),
                    ));
                }
            }
            return Ok((h, receipt));
        }
        let before = load(&self.conn, &project_s, handoff_id)?
            .ok_or_else(|| Error::NotFound("handoff not found".into()))?;
        let next = f(&before)?;
        persist(&self.conn, &project_s, &next)?;
        let receipt = record(&self.conn, &project_s, &next, request_key, op)?;
        Ok((next, receipt))
    }
}

fn load(conn: &rusqlite::Connection, project: &str, id: &str) -> Result<Option<TeamHandoff>> {
    let row = conn
        .query_row(
            "SELECT body_json FROM team_handoffs WHERE project_id=?1 AND id=?2",
            params![project, id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    match row {
        None => Ok(None),
        Some(json) => {
            Ok(Some(serde_json::from_str(&json).map_err(|e| {
                Error::Storage(format!("corrupt team handoff: {e}"))
            })?))
        }
    }
}

fn persist(conn: &rusqlite::Connection, project: &str, h: &TeamHandoff) -> Result<()> {
    let body = serde_json::to_string(h).map_err(|e| Error::Storage(e.to_string()))?;
    conn.execute(
        "INSERT INTO team_handoffs(
            project_id,id,work_item_id,kind,status,version,from_person_id,to_person_id,
            body_json,accept_request_key,created_at_ms,updated_at_ms)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(project_id,id) DO UPDATE SET
            status=excluded.status,
            version=excluded.version,
            body_json=excluded.body_json,
            accept_request_key=excluded.accept_request_key,
            updated_at_ms=excluded.updated_at_ms",
        params![
            project,
            h.id,
            h.work_item_id,
            match h.kind {
                awr_core::HandoffKind::Execution => "execution",
                awr_core::HandoffKind::Responsibility => "responsibility",
            },
            match h.status {
                HandoffStatus::Proposed => "proposed",
                HandoffStatus::Inspected => "inspected",
                HandoffStatus::Accepted => "accepted",
                HandoffStatus::Rejected => "rejected",
                HandoffStatus::Cancelled => "cancelled",
                HandoffStatus::TimedOut => "timed_out",
            },
            h.version as i64,
            h.from_person_id.as_str(),
            h.to_person_id.as_str(),
            body,
            h.accept_request_key,
            h.created_at_ms,
            h.updated_at_ms,
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

fn load_receipt(
    conn: &rusqlite::Connection,
    project: &str,
    request_key: &str,
    op: &str,
) -> Result<Option<HandoffReceipt>> {
    let row = conn
        .query_row(
            "SELECT handoff_id, event_id, op FROM team_handoff_receipts
             WHERE project_id=?1 AND request_key=?2",
            params![project, request_key],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    let Some((handoff_id, event_id, stored_op)) = row else {
        return Ok(None);
    };
    if stored_op != op {
        return Err(Error::InvalidInput(format!(
            "request_key bound to op {stored_op}, not {op}"
        )));
    }
    let h = load(conn, project, &handoff_id)?
        .ok_or_else(|| Error::Storage("handoff missing for receipt".into()))?;
    let duty = h.duty_at(h.updated_at_ms)?;
    Ok(Some(HandoffReceipt {
        request_key: request_key.into(),
        handoff_id,
        op: op.into(),
        event_id,
        replayed: true,
        status: h.status,
        version: h.version,
        duty,
    }))
}

fn record(
    conn: &rusqlite::Connection,
    project: &str,
    h: &TeamHandoff,
    request_key: &str,
    op: &str,
) -> Result<HandoffReceipt> {
    let event_id = Id::new().to_string();
    conn.execute(
        "INSERT INTO team_handoff_receipts(
            project_id,request_key,handoff_id,op,event_id,replayed,created_at_ms)
         VALUES(?1,?2,?3,?4,?5,0,?6)",
        params![project, request_key, h.id, op, event_id, h.updated_at_ms],
    )
    .map_err(db_error)?;
    let duty = h.duty_at(h.updated_at_ms)?;
    Ok(HandoffReceipt {
        request_key: request_key.into(),
        handoff_id: h.id.clone(),
        op: op.into(),
        event_id,
        replayed: false,
        status: h.status,
        version: h.version,
        duty,
    })
}
