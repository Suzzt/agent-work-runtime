//! SQLite persistence for agent authorizations (WS-016).
use crate::{Store, db_error};
use awr_core::{
    AgentAuthorization, AuthorizationStatus, ClaimEligibilityExplanation, ClaimEvaluationInput,
    DelegateAuthorizationRequest, Error, ExecutionSubjectKind, Id, IssueAuthorizationRequest,
    PersonId, Result, RevokeAuthorizationRequest, apply_delegate, apply_revoke,
    explain_claim_eligibility, now_millis, require_authorization_project, validate_issue,
};
use rusqlite::{OptionalExtension, params};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuthorizationReceipt {
    pub request_key: String,
    pub authorization_id: String,
    pub op: &'static str,
    pub event_id: Id,
    pub replayed: bool,
}

fn subject_kind_str(kind: ExecutionSubjectKind) -> &'static str {
    match kind {
        ExecutionSubjectKind::Person => "person",
        ExecutionSubjectKind::Agent => "agent",
        ExecutionSubjectKind::PlatformService => "platform_service",
    }
}

fn status_str(status: AuthorizationStatus) -> &'static str {
    match status {
        AuthorizationStatus::Active => "active",
        AuthorizationStatus::Revoked => "revoked",
        AuthorizationStatus::Expired => "expired",
    }
}

fn load_auth(
    conn: &rusqlite::Connection,
    project: &str,
    id: &str,
) -> Result<Option<AgentAuthorization>> {
    let row = conn
        .query_row(
            "SELECT body_json FROM agent_authorizations WHERE project_id=?1 AND id=?2",
            params![project, id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    match row {
        None => Ok(None),
        Some(json) => Ok(Some(serde_json::from_str(&json).map_err(|e| {
            Error::Storage(format!("corrupt agent authorization: {e}"))
        })?)),
    }
}

fn persist_auth(
    conn: &rusqlite::Connection,
    project: &str,
    auth: &AgentAuthorization,
) -> Result<()> {
    let body = serde_json::to_string(auth).map_err(|e| Error::Storage(e.to_string()))?;
    conn.execute(
        "INSERT INTO agent_authorizations(
            project_id,id,authorizer_person_id,responsible_person_id,subject_kind,subject_id,
            client_id,session_id,model_id,status,expires_at_ms,revoked_at_ms,revoked_by,
            parent_authorization_id,maintainer_person_id,binding_id,created_at_ms,body_json)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
         ON CONFLICT(project_id,id) DO UPDATE SET
            status=excluded.status,
            expires_at_ms=excluded.expires_at_ms,
            revoked_at_ms=excluded.revoked_at_ms,
            revoked_by=excluded.revoked_by,
            parent_authorization_id=excluded.parent_authorization_id,
            body_json=excluded.body_json",
        params![
            project,
            auth.id,
            auth.authorizer_person_id.as_str(),
            auth.responsible_person_id.as_str(),
            subject_kind_str(auth.subject_kind),
            auth.subject_id,
            auth.client_id,
            auth.session_id,
            auth.model_id,
            status_str(auth.status),
            auth.expires_at_ms,
            auth.revoked_at_ms,
            auth.revoked_by.as_ref().map(|p| p.as_str().to_string()),
            auth.parent_authorization_id,
            auth.maintainer_person_id
                .as_ref()
                .map(|p| p.as_str().to_string()),
            auth.binding_id,
            auth.created_at_ms,
            body,
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

fn load_receipt(
    conn: &rusqlite::Connection,
    project: &str,
    request_key: &str,
) -> Result<Option<AuthorizationReceipt>> {
    conn.query_row(
        "SELECT authorization_id,op,event_id FROM agent_authorization_receipts
         WHERE project_id=?1 AND request_key=?2",
        params![project, request_key],
        |r| {
            let op = r.get::<_, String>(1)?;
            Ok(AuthorizationReceipt {
                request_key: request_key.into(),
                authorization_id: r.get(0)?,
                op: match op.as_str() {
                    "issue" => "issue",
                    "revoke" => "revoke",
                    "delegate" => "delegate",
                    _ => "unknown",
                },
                event_id: r
                    .get::<_, String>(2)?
                    .parse()
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?,
                replayed: false,
            })
        },
    )
    .optional()
    .map_err(db_error)
}

fn save_receipt(
    conn: &rusqlite::Connection,
    project: &str,
    request_key: &str,
    authorization_id: &str,
    op: &str,
    event_id: Id,
    now: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO agent_authorization_receipts(project_id,request_key,authorization_id,op,event_id,replayed,created_at_ms)
         VALUES(?1,?2,?3,?4,?5,0,?6)",
        params![
            project,
            request_key,
            authorization_id,
            op,
            event_id.to_string(),
            now
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

impl Store {
    pub fn get_agent_authorization(
        &self,
        project: Id,
        authorization_id: &str,
    ) -> Result<Option<AgentAuthorization>> {
        load_auth(&self.conn, &project.to_string(), authorization_id)
    }

    pub fn list_agent_authorizations(
        &self,
        project: Id,
        responsible_person: Option<&PersonId>,
        subject_id: Option<&str>,
        active_only: bool,
    ) -> Result<Vec<AgentAuthorization>> {
        let project = project.to_string();
        let rows = self
            .conn
            .prepare(
                "SELECT body_json, responsible_person_id, subject_id, status
                 FROM agent_authorizations WHERE project_id=?1 ORDER BY created_at_ms ASC",
            )
            .map_err(db_error)?
            .query_map(params![project], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        let mut out = Vec::new();
        for (json, responsible, subject, status) in rows {
            if let Some(person) = responsible_person {
                if responsible != person.as_str() {
                    continue;
                }
            }
            if let Some(sid) = subject_id {
                if subject != sid {
                    continue;
                }
            }
            if active_only && status != "active" {
                continue;
            }
            out.push(
                serde_json::from_str(&json)
                    .map_err(|e| Error::Storage(format!("corrupt agent authorization: {e}")))?,
            );
        }
        Ok(out)
    }

    pub fn issue_agent_authorization(
        &mut self,
        project: Id,
        req: &IssueAuthorizationRequest,
    ) -> Result<(AgentAuthorization, AuthorizationReceipt)> {
        validate_issue(req)?;
        require_authorization_project(&req.authorization, &project.to_string())?;
        let project_s = project.to_string();
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        if let Some(mut receipt) = load_receipt(&tx, &project_s, &req.request_key)? {
            let auth = load_auth(&tx, &project_s, &receipt.authorization_id)?
                .ok_or_else(|| Error::NotFound("authorization missing for receipt".into()))?;
            if receipt.op != "issue"
                || receipt.authorization_id != req.authorization.id
                || auth != req.authorization
            {
                return Err(Error::RuleViolation(
                    "request key was already used for a different authorization operation".into(),
                ));
            }
            receipt.replayed = true;
            tx.commit().map_err(db_error)?;
            return Ok((auth, receipt));
        }
        if load_auth(&tx, &project_s, &req.authorization.id)?.is_some() {
            return Err(Error::InvalidInput(
                "authorization id already exists; use a new id or revoke the existing grant".into(),
            ));
        }
        persist_auth(&tx, &project_s, &req.authorization)?;
        let event_id = Id::new();
        let now = now_millis()?;
        save_receipt(
            &tx,
            &project_s,
            &req.request_key,
            &req.authorization.id,
            "issue",
            event_id,
            now,
        )?;
        tx.commit().map_err(db_error)?;
        Ok((
            req.authorization.clone(),
            AuthorizationReceipt {
                request_key: req.request_key.clone(),
                authorization_id: req.authorization.id.clone(),
                op: "issue",
                event_id,
                replayed: false,
            },
        ))
    }

    pub fn revoke_agent_authorization(
        &mut self,
        project: Id,
        req: &RevokeAuthorizationRequest,
    ) -> Result<(AgentAuthorization, AuthorizationReceipt)> {
        let project_s = project.to_string();
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        if let Some(mut receipt) = load_receipt(&tx, &project_s, &req.request_key)? {
            let auth = load_auth(&tx, &project_s, &receipt.authorization_id)?
                .ok_or_else(|| Error::NotFound("authorization missing for receipt".into()))?;
            if receipt.op != "revoke"
                || receipt.authorization_id != req.authorization_id
                || auth.revoked_by.as_ref() != Some(&req.revoked_by)
                || auth.revoked_at_ms != Some(req.revoked_at_ms)
            {
                return Err(Error::RuleViolation(
                    "request key was already used for a different authorization operation".into(),
                ));
            }
            receipt.replayed = true;
            tx.commit().map_err(db_error)?;
            return Ok((auth, receipt));
        }
        let current = load_auth(&tx, &project_s, &req.authorization_id)?
            .ok_or_else(|| Error::NotFound("authorization not found".into()))?;
        let next = apply_revoke(&current, req)?;
        persist_auth(&tx, &project_s, &next)?;
        let event_id = Id::new();
        let now = now_millis()?;
        save_receipt(
            &tx,
            &project_s,
            &req.request_key,
            &next.id,
            "revoke",
            event_id,
            now,
        )?;
        tx.commit().map_err(db_error)?;
        Ok((
            next,
            AuthorizationReceipt {
                request_key: req.request_key.clone(),
                authorization_id: req.authorization_id.clone(),
                op: "revoke",
                event_id,
                replayed: false,
            },
        ))
    }

    pub fn delegate_agent_authorization(
        &mut self,
        project: Id,
        req: &DelegateAuthorizationRequest,
        now_ms: i64,
    ) -> Result<(AgentAuthorization, AuthorizationReceipt)> {
        let project_s = project.to_string();
        require_authorization_project(&req.child, &project_s)?;
        let tx = self.conn.unchecked_transaction().map_err(db_error)?;
        if let Some(mut receipt) = load_receipt(&tx, &project_s, &req.request_key)? {
            let auth = load_auth(&tx, &project_s, &receipt.authorization_id)?
                .ok_or_else(|| Error::NotFound("authorization missing for receipt".into()))?;
            let mut expected = req.child.clone();
            expected.parent_authorization_id = Some(req.parent_authorization_id.clone());
            if receipt.op != "delegate"
                || receipt.authorization_id != expected.id
                || auth != expected
            {
                return Err(Error::RuleViolation(
                    "request key was already used for a different authorization operation".into(),
                ));
            }
            receipt.replayed = true;
            tx.commit().map_err(db_error)?;
            return Ok((auth, receipt));
        }
        let parent = load_auth(&tx, &project_s, &req.parent_authorization_id)?
            .ok_or_else(|| Error::NotFound("parent authorization not found".into()))?;
        let child = apply_delegate(&parent, req, now_ms)?;
        if load_auth(&tx, &project_s, &child.id)?.is_some() {
            return Err(Error::InvalidInput(
                "child authorization id already exists".into(),
            ));
        }
        persist_auth(&tx, &project_s, &child)?;
        let event_id = Id::new();
        let now = now_millis()?;
        save_receipt(
            &tx,
            &project_s,
            &req.request_key,
            &child.id,
            "delegate",
            event_id,
            now,
        )?;
        tx.commit().map_err(db_error)?;
        Ok((
            child.clone(),
            AuthorizationReceipt {
                request_key: req.request_key.clone(),
                authorization_id: child.id,
                op: "delegate",
                event_id,
                replayed: false,
            },
        ))
    }

    pub fn explain_claim(
        &self,
        input: &ClaimEvaluationInput<'_>,
    ) -> Result<ClaimEligibilityExplanation> {
        explain_claim_eligibility(input)
    }
}
