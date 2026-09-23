//! SQLite persistence for versioned delivery dependencies and adoption credentials (WS-030).
use crate::{Store, db_error};
use awr_core::{
    AdoptDeliveryRequest, AdoptionCredential, DeliveryCredentialReceipt, DeliveryError, Error,
    ExportAuthorization, GrantExportAuthorizationRequest, HardDeliveryDependency, Id,
    RegisterHardDependencyRequest, Result, RevokeExportAuthorizationRequest,
    RevokeHardDependencyRequest, adopt_delivery_credential as core_adopt_delivery_credential,
    apply_export_revoke, apply_hard_dependency_revoke, validate_export_grant,
    validate_hard_dependency_registration,
};
use rusqlite::{OptionalExtension, params};

fn map_delivery(err: DeliveryError) -> Error {
    match err {
        DeliveryError::InvalidDefinition => {
            Error::InvalidInput("invalid delivery definition".into())
        }
        DeliveryError::BindingMismatch => Error::InvalidInput("delivery binding mismatch".into()),
        DeliveryError::InvalidTime => Error::InvalidInput("invalid delivery time".into()),
        DeliveryError::NotSatisfied(a) => Error::RuleViolation(format!(
            "delivery not satisfied: {:?}/{:?}",
            a.status, a.reason
        )),
    }
}

fn load_dep(
    conn: &rusqlite::Connection,
    project: &str,
    id: &str,
) -> Result<Option<HardDeliveryDependency>> {
    let row = conn
        .query_row(
            "SELECT body_json FROM hard_delivery_dependencies WHERE project_id=?1 AND id=?2",
            params![project, id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    match row {
        None => Ok(None),
        Some(json) => Ok(Some(serde_json::from_str(&json).map_err(|e| {
            Error::Storage(format!("corrupt hard delivery dependency: {e}"))
        })?)),
    }
}

fn persist_dep(
    conn: &rusqlite::Connection,
    project: &str,
    dep: &HardDeliveryDependency,
) -> Result<()> {
    let body = serde_json::to_string(dep).map_err(|e| Error::Storage(e.to_string()))?;
    let policy = match dep.policy {
        awr_core::DeliveryVersionPolicy::FixedDelivery => "fixed_delivery",
        awr_core::DeliveryVersionPolicy::CurrentContract => "current_contract",
    };
    let status = match dep.status {
        awr_core::HardDependencyStatus::Active => "active",
        awr_core::HardDependencyStatus::Revoked => "revoked",
    };
    conn.execute(
        "INSERT INTO hard_delivery_dependencies(
            project_id,id,provider_work_item_id,provider_workstream_id,
            consumer_work_item_id,consumer_workstream_id,policy,status,
            completion_receipt,contract_sha256,artifact_sha256,created_at_ms,revoked_at_ms,body_json)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
         ON CONFLICT(project_id,id) DO UPDATE SET
            status=excluded.status,
            revoked_at_ms=excluded.revoked_at_ms,
            body_json=excluded.body_json",
        params![
            project,
            dep.id,
            dep.provider.work_item_id,
            dep.provider.workstream_id.to_string(),
            dep.consumer.work_item_id,
            dep.consumer.workstream_id.to_string(),
            policy,
            status,
            dep.selected.completion_receipt.to_string(),
            dep.selected.contract_sha256,
            dep.selected.artifact_sha256,
            dep.created_at_ms,
            dep.revoked_at_ms,
            body,
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

fn load_export(
    conn: &rusqlite::Connection,
    project: &str,
    id: &str,
) -> Result<Option<ExportAuthorization>> {
    let row = conn
        .query_row(
            "SELECT body_json FROM export_authorizations WHERE project_id=?1 AND id=?2",
            params![project, id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    match row {
        None => Ok(None),
        Some(json) => Ok(Some(serde_json::from_str(&json).map_err(|e| {
            Error::Storage(format!("corrupt export authorization: {e}"))
        })?)),
    }
}

fn persist_export(
    conn: &rusqlite::Connection,
    project: &str,
    auth: &ExportAuthorization,
) -> Result<()> {
    let body = serde_json::to_string(auth).map_err(|e| Error::Storage(e.to_string()))?;
    let status = match auth.status {
        awr_core::ExportAuthorizationStatus::Granted => "granted",
        awr_core::ExportAuthorizationStatus::Denied => "denied",
        awr_core::ExportAuthorizationStatus::Revoked => "revoked",
    };
    conn.execute(
        "INSERT INTO export_authorizations(
            project_id,id,provider_work_item_id,status,completion_receipt,contract_sha256,
            artifact_sha256,export_scope_sha256,granted_by,created_at_ms,revoked_at_ms,body_json)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(project_id,id) DO UPDATE SET
            status=excluded.status,
            revoked_at_ms=excluded.revoked_at_ms,
            body_json=excluded.body_json",
        params![
            project,
            auth.id,
            auth.provider_work_item_id,
            status,
            auth.delivery.completion_receipt.to_string(),
            auth.delivery.contract_sha256,
            auth.delivery.artifact_sha256,
            auth.delivery.export_scope_sha256,
            auth.granted_by,
            auth.created_at_ms,
            auth.revoked_at_ms,
            body,
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

fn load_credential(
    conn: &rusqlite::Connection,
    project: &str,
    id: &str,
) -> Result<Option<AdoptionCredential>> {
    let row = conn
        .query_row(
            "SELECT body_json FROM adoption_credentials WHERE project_id=?1 AND id=?2",
            params![project, id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    match row {
        None => Ok(None),
        Some(json) => Ok(Some(serde_json::from_str(&json).map_err(|e| {
            Error::Storage(format!("corrupt adoption credential: {e}"))
        })?)),
    }
}

fn persist_credential(
    conn: &rusqlite::Connection,
    project: &str,
    cred: &AdoptionCredential,
) -> Result<()> {
    let body = serde_json::to_string(cred).map_err(|e| Error::Storage(e.to_string()))?;
    let status = match cred.status {
        awr_core::AdoptionCredentialStatus::Active => "active",
        awr_core::AdoptionCredentialStatus::Stale => "stale",
        awr_core::AdoptionCredentialStatus::Revoked => "revoked",
    };
    conn.execute(
        "INSERT INTO adoption_credentials(
            project_id,id,dependency_id,status,completion_receipt,export_authorization_id,
            adopted_at_ms,body_json)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(project_id,id) DO UPDATE SET
            status=excluded.status,
            body_json=excluded.body_json",
        params![
            project,
            cred.id,
            cred.dependency_id,
            status,
            cred.completion_receipt_id.to_string(),
            cred.export_authorization_id,
            cred.adopted_at_ms,
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
    op: &str,
) -> Result<Option<DeliveryCredentialReceipt>> {
    let row = conn
        .query_row(
            "SELECT subject_id,op,event_id,replayed FROM delivery_credential_receipts
             WHERE project_id=?1 AND request_key=?2",
            params![project, request_key],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    match row {
        None => Ok(None),
        Some((subject_id, stored_op, event_id, replayed)) => {
            if stored_op != op {
                return Err(Error::InvalidInput(
                    "delivery credential request_key reused with different op".into(),
                ));
            }
            Ok(Some(DeliveryCredentialReceipt {
                request_key: request_key.into(),
                subject_id,
                op: stored_op,
                event_id: event_id
                    .parse()
                    .map_err(|_| Error::Storage("receipt event id is not a ulid".into()))?,
                replayed: replayed != 0,
            }))
        }
    }
}

fn record_receipt(
    conn: &rusqlite::Connection,
    project: &str,
    request_key: &str,
    subject_id: &str,
    op: &str,
) -> Result<DeliveryCredentialReceipt> {
    let event_id = Id::new();
    conn.execute(
        "INSERT INTO delivery_credential_receipts(
            project_id,request_key,subject_id,op,event_id,replayed,created_at_ms)
         VALUES(?1,?2,?3,?4,?5,0,?6)",
        params![
            project,
            request_key,
            subject_id,
            op,
            event_id.to_string(),
            awr_core::now_millis()?,
        ],
    )
    .map_err(db_error)?;
    Ok(DeliveryCredentialReceipt {
        request_key: request_key.into(),
        subject_id: subject_id.into(),
        op: op.into(),
        event_id,
        replayed: false,
    })
}

impl Store {
    pub fn get_hard_delivery_dependency(
        &self,
        project: Id,
        dependency_id: &str,
    ) -> Result<Option<HardDeliveryDependency>> {
        load_dep(&self.conn, &project.to_string(), dependency_id)
    }

    pub fn register_hard_delivery_dependency(
        &mut self,
        project: Id,
        req: &RegisterHardDependencyRequest,
    ) -> Result<(HardDeliveryDependency, DeliveryCredentialReceipt)> {
        let project_s = project.to_string();
        if project_s != req.provider.project_id || project_s != req.consumer.project_id {
            return Err(Error::InvalidInput(
                "delivery dependency project must match store project".into(),
            ));
        }
        if let Some(receipt) = load_receipt(
            &self.conn,
            &project_s,
            &req.request_key,
            "register_dependency",
        )? {
            let dep = load_dep(&self.conn, &project_s, &receipt.subject_id)?
                .ok_or_else(|| Error::Storage("dependency missing for receipt".into()))?;
            return Ok((
                dep,
                DeliveryCredentialReceipt {
                    replayed: true,
                    ..receipt
                },
            ));
        }
        if load_dep(&self.conn, &project_s, &req.dependency_id)?.is_some() {
            return Err(Error::InvalidInput(
                "hard delivery dependency id already registered".into(),
            ));
        }
        let dep = validate_hard_dependency_registration(req).map_err(map_delivery)?;
        persist_dep(&self.conn, &project_s, &dep)?;
        let receipt = record_receipt(
            &self.conn,
            &project_s,
            &req.request_key,
            &dep.id,
            "register_dependency",
        )?;
        Ok((dep, receipt))
    }

    pub fn revoke_hard_delivery_dependency(
        &mut self,
        project: Id,
        req: &RevokeHardDependencyRequest,
    ) -> Result<(HardDeliveryDependency, DeliveryCredentialReceipt)> {
        let project_s = project.to_string();
        if let Some(receipt) = load_receipt(
            &self.conn,
            &project_s,
            &req.request_key,
            "revoke_dependency",
        )? {
            let dep = load_dep(&self.conn, &project_s, &receipt.subject_id)?
                .ok_or_else(|| Error::Storage("dependency missing for receipt".into()))?;
            return Ok((
                dep,
                DeliveryCredentialReceipt {
                    replayed: true,
                    ..receipt
                },
            ));
        }
        let current = load_dep(&self.conn, &project_s, &req.dependency_id)?
            .ok_or_else(|| Error::NotFound("hard delivery dependency missing".into()))?;
        let dep = apply_hard_dependency_revoke(&current, req).map_err(map_delivery)?;
        persist_dep(&self.conn, &project_s, &dep)?;
        let receipt = record_receipt(
            &self.conn,
            &project_s,
            &req.request_key,
            &dep.id,
            "revoke_dependency",
        )?;
        Ok((dep, receipt))
    }

    pub fn get_export_authorization(
        &self,
        project: Id,
        authorization_id: &str,
    ) -> Result<Option<ExportAuthorization>> {
        load_export(&self.conn, &project.to_string(), authorization_id)
    }

    pub fn grant_export_authorization(
        &mut self,
        project: Id,
        req: &GrantExportAuthorizationRequest,
    ) -> Result<(ExportAuthorization, DeliveryCredentialReceipt)> {
        let project_s = project.to_string();
        if project_s != req.project_id {
            return Err(Error::InvalidInput(
                "export authorization project must match store project".into(),
            ));
        }
        if let Some(receipt) =
            load_receipt(&self.conn, &project_s, &req.request_key, "grant_export")?
        {
            let auth = load_export(&self.conn, &project_s, &receipt.subject_id)?
                .ok_or_else(|| Error::Storage("export authorization missing for receipt".into()))?;
            return Ok((
                auth,
                DeliveryCredentialReceipt {
                    replayed: true,
                    ..receipt
                },
            ));
        }
        if load_export(&self.conn, &project_s, &req.authorization_id)?.is_some() {
            return Err(Error::InvalidInput(
                "export authorization id already registered".into(),
            ));
        }
        let auth = validate_export_grant(req).map_err(map_delivery)?;
        persist_export(&self.conn, &project_s, &auth)?;
        let receipt = record_receipt(
            &self.conn,
            &project_s,
            &req.request_key,
            &auth.id,
            "grant_export",
        )?;
        Ok((auth, receipt))
    }

    pub fn revoke_export_authorization(
        &mut self,
        project: Id,
        req: &RevokeExportAuthorizationRequest,
    ) -> Result<(ExportAuthorization, DeliveryCredentialReceipt)> {
        let project_s = project.to_string();
        if let Some(receipt) =
            load_receipt(&self.conn, &project_s, &req.request_key, "revoke_export")?
        {
            let auth = load_export(&self.conn, &project_s, &receipt.subject_id)?
                .ok_or_else(|| Error::Storage("export authorization missing for receipt".into()))?;
            return Ok((
                auth,
                DeliveryCredentialReceipt {
                    replayed: true,
                    ..receipt
                },
            ));
        }
        let current = load_export(&self.conn, &project_s, &req.authorization_id)?
            .ok_or_else(|| Error::NotFound("export authorization missing".into()))?;
        let auth = apply_export_revoke(&current, req).map_err(map_delivery)?;
        persist_export(&self.conn, &project_s, &auth)?;
        let receipt = record_receipt(
            &self.conn,
            &project_s,
            &req.request_key,
            &auth.id,
            "revoke_export",
        )?;
        Ok((auth, receipt))
    }

    pub fn get_adoption_credential(
        &self,
        project: Id,
        credential_id: &str,
    ) -> Result<Option<AdoptionCredential>> {
        load_credential(&self.conn, &project.to_string(), credential_id)
    }

    pub fn adopt_delivery_credential(
        &mut self,
        project: Id,
        req: &AdoptDeliveryRequest,
    ) -> Result<(AdoptionCredential, DeliveryCredentialReceipt)> {
        let project_s = project.to_string();
        if project_s != req.dependency.provider.project_id
            || project_s != req.dependency.consumer.project_id
        {
            return Err(Error::InvalidInput(
                "adoption credential project must match store project".into(),
            ));
        }
        if let Some(receipt) = load_receipt(&self.conn, &project_s, &req.request_key, "adopt")? {
            let cred = load_credential(&self.conn, &project_s, &receipt.subject_id)?
                .ok_or_else(|| Error::Storage("adoption credential missing for receipt".into()))?;
            return Ok((
                cred,
                DeliveryCredentialReceipt {
                    replayed: true,
                    ..receipt
                },
            ));
        }
        let stored_dep = load_dep(&self.conn, &project_s, &req.dependency.id)?
            .ok_or_else(|| Error::NotFound("hard delivery dependency missing".into()))?;
        if stored_dep != req.dependency {
            return Err(Error::InvalidInput(
                "adoption dependency snapshot does not match store".into(),
            ));
        }
        let stored_export = load_export(&self.conn, &project_s, &req.export_authorization.id)?
            .ok_or_else(|| Error::NotFound("export authorization missing".into()))?;
        if stored_export != req.export_authorization {
            return Err(Error::InvalidInput(
                "adoption export authorization snapshot does not match store".into(),
            ));
        }
        if load_credential(&self.conn, &project_s, &req.credential_id)?.is_some() {
            return Err(Error::InvalidInput(
                "adoption credential id already registered".into(),
            ));
        }
        // Residual trust boundary: SQLite has no WS-018 completion_receipts /
        // work_runtime selection tables. Team PG (DeliveryAdoptionStore::adopt)
        // loads the trusted completion proof from storage before issuing a
        // credential; this local path still passes request-supplied proof
        // through core consistency checks only.
        let cred = core_adopt_delivery_credential(req).map_err(map_delivery)?;
        persist_credential(&self.conn, &project_s, &cred)?;
        let receipt = record_receipt(&self.conn, &project_s, &req.request_key, &cred.id, "adopt")?;
        Ok((cred, receipt))
    }
}
