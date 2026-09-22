//! AWR-TMCP-040: permission / planning / delivery operations audit.
//!
//! Success rows are inserted in the **same** business transaction as events and
//! receipts. Deny rows use a separate, capacity-bounded, redacted channel that
//! never mutates business state. Reads (history / count / export) are authorized:
//! project-wide only with `audit.read_project`; otherwise a member sees only their
//! own actor records.
//!
//! Out of scope: full chat text, arbitrary tool I/O, token billing (WS-041).
//! This PG audit does **not** claim protection against database-owner tampering
//! or enterprise non-repudiation / WORM storage.

use crate::tx::new_id;
use crate::workstream_auth::{ReaderAuthority, authenticate, authorize_domain_action};
use crate::{PgError, PgPool, PgResult};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_postgres::Transaction;

/// Soft capacity for deny rows retained per project (oldest pruned).
pub const DENY_CAPACITY_PER_PROJECT: i64 = 1000;
/// Hard cap on a single export/history page.
pub const HISTORY_PAGE_MAX: i64 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpsCategory {
    Access,
    Planning,
    Delivery,
    Other,
}

impl OpsCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Access => "access",
            Self::Planning => "planning",
            Self::Delivery => "delivery",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Debug)]
pub struct OpsAuditWrite {
    pub category: OpsCategory,
    pub action: String,
    pub result: &'static str,
    pub person_id: Option<String>,
    pub actor_id: String,
    pub client_id: String,
    pub target_kind: String,
    pub target_id: Option<String>,
    pub work_id: Option<String>,
    pub change_id: Option<String>,
    pub request_id: Option<String>,
    pub membership_version: Option<i64>,
    pub authority_version: Option<i64>,
    pub policy_version: Option<i32>,
    pub source_version: Option<String>,
    pub digest: Option<String>,
    pub summary: Value,
}

#[derive(Clone, Debug)]
pub struct OpsDenyWrite {
    pub category: OpsCategory,
    pub action: String,
    pub actor_id: Option<String>,
    pub client_id: Option<String>,
    pub person_id: Option<String>,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub request_id: Option<String>,
    pub reason_code: String,
}

/// Redact a summary map: drop known secret-bearing keys and truncate strings.
pub fn redact_summary(mut summary: Value) -> Value {
    const DROP: &[&str] = &[
        "secret",
        "secret_hash",
        "token",
        "bearer",
        "password",
        "raw_secret",
        "chat",
        "messages",
        "tool_input",
        "tool_output",
        "token_usage",
        "prompt",
        "completion",
    ];
    if let Some(obj) = summary.as_object_mut() {
        obj.retain(|k, _| {
            let lower = k.to_ascii_lowercase();
            !DROP.iter().any(|d| lower.contains(d))
        });
        for v in obj.values_mut() {
            if let Some(s) = v.as_str() {
                if s.len() > 512 {
                    *v = Value::String(format!("{}…", &s[..509]));
                }
            }
        }
    }
    summary
}

pub fn digest_of(v: &Value) -> String {
    let bytes = serde_json::to_vec(v).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

fn sanitize_reason(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "denied".into();
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("secret")
        || lower.contains("token")
        || lower.contains("bearer")
        || lower.contains("password")
    {
        return "redacted_deny".into();
    }
    trimmed.chars().take(128).collect()
}

/// Insert one ops-audit success row inside an open business transaction.
pub async fn record_in_tx(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    write: &OpsAuditWrite,
) -> PgResult<String> {
    if write.category == OpsCategory::Other {
        return Err(PgError::Protocol(
            "ops audit success rows require access|planning|delivery".into(),
        ));
    }
    if write.result != "committed" && write.result != "succeeded" {
        return Err(PgError::Protocol("invalid ops audit result".into()));
    }
    let id = new_id();
    let summary = redact_summary(write.summary.clone());
    tx.execute(
        "INSERT INTO awr_team.ops_audit_records(
            tenant_id, project_id, id, category, action, result,
            person_id, actor_id, client_id, target_kind, target_id,
            work_id, change_id, request_id, membership_version, authority_version,
            policy_version, source_version, digest, summary_json)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)",
        &[
            &tenant_id,
            &project_id,
            &id,
            &write.category.as_str(),
            &write.action,
            &write.result,
            &write.person_id,
            &write.actor_id,
            &write.client_id,
            &write.target_kind,
            &write.target_id,
            &write.work_id,
            &write.change_id,
            &write.request_id,
            &write.membership_version,
            &write.authority_version,
            &write.policy_version,
            &write.source_version,
            &write.digest,
            &summary,
        ],
    )
    .await?;
    Ok(id)
}

/// Record a redacted deny on a capacity-bounded channel. Opens its own
/// transaction and never touches business tables.
pub async fn record_deny(
    pool: &PgPool,
    tenant_id: &str,
    project_id: &str,
    write: &OpsDenyWrite,
) -> PgResult<String> {
    let mut client = pool.get().await?;
    crate::check_schema(&client).await?;
    let tx = client.transaction().await?;
    crate::tx::bind_workstream_scope(&tx, tenant_id, project_id).await?;
    let exists = tx
        .query_opt(
            "SELECT 1 FROM awr_team.projects WHERE tenant_id=$1 AND id=$2",
            &[&tenant_id, &project_id],
        )
        .await?;
    if exists.is_none() {
        return Err(PgError::ProjectNotAvailable);
    }
    let id = new_id();
    let reason = sanitize_reason(&write.reason_code);
    tx.execute(
        "INSERT INTO awr_team.ops_audit_denies(
            tenant_id, project_id, id, category, action,
            actor_id, client_id, person_id, target_kind, target_id,
            request_id, reason_code)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        &[
            &tenant_id,
            &project_id,
            &id,
            &write.category.as_str(),
            &write.action,
            &write.actor_id,
            &write.client_id,
            &write.person_id,
            &write.target_kind,
            &write.target_id,
            &write.request_id,
            &reason,
        ],
    )
    .await?;
    tx.execute(
        "DELETE FROM awr_team.ops_audit_denies d
         WHERE d.tenant_id=$1 AND d.project_id=$2
           AND d.ctid IN (
             SELECT ctid FROM awr_team.ops_audit_denies
             WHERE tenant_id=$1 AND project_id=$2
             ORDER BY created_at DESC, id DESC
             OFFSET $3
           )",
        &[&tenant_id, &project_id, &DENY_CAPACITY_PER_PROJECT],
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

#[derive(Clone, Debug, Default)]
pub struct OpsHistoryFilter {
    pub work_id: Option<String>,
    pub change_id: Option<String>,
    pub member_actor_id: Option<String>,
    pub request_id: Option<String>,
    pub category: Option<String>,
    pub include_denies: bool,
    pub limit: Option<i64>,
}

pub struct OpsAuditStore {
    pool: Arc<PgPool>,
}

impl OpsAuditStore {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            pool: Arc::new(PgPool::new(url)),
        }
    }

    pub fn from_pool(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    pub fn from_config(config: tokio_postgres::Config) -> Self {
        Self {
            pool: Arc::new(PgPool::from_config(config)),
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn history(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        filter: &OpsHistoryFilter,
    ) -> PgResult<Value> {
        self.read_surface(tenant_id, project_id, bearer, filter, false, false)
            .await
    }

    pub async fn export(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        filter: &OpsHistoryFilter,
    ) -> PgResult<Value> {
        self.read_surface(tenant_id, project_id, bearer, filter, false, true)
            .await
    }

    pub async fn count(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        filter: &OpsHistoryFilter,
    ) -> PgResult<Value> {
        self.read_surface(tenant_id, project_id, bearer, filter, true, false)
            .await
    }

    async fn read_surface(
        &self,
        tenant_id: &str,
        project_id: &str,
        bearer: &str,
        filter: &OpsHistoryFilter,
        count_only: bool,
        as_export: bool,
    ) -> PgResult<Value> {
        let mut client = self.pool.get().await?;
        crate::check_schema(&client).await?;
        let tx = client.transaction().await?;
        let auth = authenticate(&tx, tenant_id, project_id, bearer).await?;
        let project_wide =
            authorize_domain_action(&auth, awr_team::Action::AuditReadProject, None, None).is_ok();
        if !project_wide {
            authorize_domain_action(&auth, awr_team::Action::WorkRead, None, None)?;
            if let Some(ref m) = filter.member_actor_id {
                if m != &auth.actor_id {
                    return Err(PgError::Forbidden);
                }
            }
        }
        let limit = filter.limit.unwrap_or(50).clamp(1, HISTORY_PAGE_MAX);
        let actor_scope = if project_wide {
            filter.member_actor_id.clone()
        } else {
            Some(auth.actor_id.clone())
        };

        if count_only {
            let n =
                count_records(&tx, tenant_id, project_id, actor_scope.as_deref(), filter).await?;
            let deny_count = if filter.include_denies {
                count_denies(&tx, tenant_id, project_id, actor_scope.as_deref(), filter).await?
            } else {
                0
            };
            tx.commit().await?;
            return Ok(json!({
                "protocol": "awr-ops-audit-v1",
                "scope": if project_wide { "project" } else { "self" },
                "records": n,
                "denies": deny_count,
                "authorized": true,
                "non_repudiation": "not_claimed_against_db_owner"
            }));
        }

        let records = list_records(
            &tx,
            tenant_id,
            project_id,
            actor_scope.as_deref(),
            filter,
            limit,
        )
        .await?;
        let denies = if filter.include_denies {
            list_denies(
                &tx,
                tenant_id,
                project_id,
                actor_scope.as_deref(),
                filter,
                limit,
            )
            .await?
        } else {
            Vec::new()
        };
        tx.commit().await?;
        let mut out = json!({
            "protocol": "awr-ops-audit-v1",
            "scope": if project_wide { "project" } else { "self" },
            "records": records,
            "denies": denies,
            "limit": limit,
            "chat_text_collected": false,
            "tool_io_collected": false,
            "token_billing_collected": false,
            "non_repudiation": "not_claimed_against_db_owner"
        });
        if as_export {
            if let Some(obj) = out.as_object_mut() {
                obj.insert("export".into(), Value::Bool(true));
                obj.insert(
                    "scope_note".into(),
                    json!("ops_audit_only_no_chat_tool_io_or_token_billing"),
                );
            }
        }
        Ok(out)
    }
}

fn row_to_record(row: &tokio_postgres::Row) -> Value {
    let created: std::time::SystemTime = row.get("created_at");
    let created_ms = created
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    json!({
        "id": row.get::<_, String>("id"),
        "category": row.get::<_, String>("category"),
        "action": row.get::<_, String>("action"),
        "result": row.get::<_, String>("result"),
        "person_id": row.get::<_, Option<String>>("person_id"),
        "actor_id": row.get::<_, String>("actor_id"),
        "client_id": row.get::<_, String>("client_id"),
        "target_kind": row.get::<_, String>("target_kind"),
        "target_id": row.get::<_, Option<String>>("target_id"),
        "work_id": row.get::<_, Option<String>>("work_id"),
        "change_id": row.get::<_, Option<String>>("change_id"),
        "request_id": row.get::<_, Option<String>>("request_id"),
        "membership_version": row.get::<_, Option<i64>>("membership_version").map(|v| v.to_string()),
        "authority_version": row.get::<_, Option<i64>>("authority_version").map(|v| v.to_string()),
        "policy_version": row.get::<_, Option<i32>>("policy_version"),
        "source_version": row.get::<_, Option<String>>("source_version"),
        "digest": row.get::<_, Option<String>>("digest"),
        "summary": row.get::<_, Value>("summary_json"),
        "created_at_unix_ms": created_ms
    })
}

async fn count_records(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    actor_scope: Option<&str>,
    filter: &OpsHistoryFilter,
) -> PgResult<i64> {
    let n: i64 = tx
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.ops_audit_records
             WHERE tenant_id=$1 AND project_id=$2
               AND ($3::text IS NULL OR actor_id=$3)
               AND ($4::text IS NULL OR work_id=$4)
               AND ($5::text IS NULL OR change_id=$5)
               AND ($6::text IS NULL OR request_id=$6)
               AND ($7::text IS NULL OR category=$7)",
            &[
                &tenant_id,
                &project_id,
                &actor_scope,
                &filter.work_id,
                &filter.change_id,
                &filter.request_id,
                &filter.category,
            ],
        )
        .await?
        .get(0);
    Ok(n)
}

async fn list_records(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    actor_scope: Option<&str>,
    filter: &OpsHistoryFilter,
    limit: i64,
) -> PgResult<Vec<Value>> {
    let rows = tx
        .query(
            "SELECT id, category, action, result, person_id, actor_id, client_id,
                    target_kind, target_id, work_id, change_id, request_id,
                    membership_version, authority_version, policy_version,
                    source_version, digest, summary_json, created_at
             FROM awr_team.ops_audit_records
             WHERE tenant_id=$1 AND project_id=$2
               AND ($3::text IS NULL OR actor_id=$3)
               AND ($4::text IS NULL OR work_id=$4)
               AND ($5::text IS NULL OR change_id=$5)
               AND ($6::text IS NULL OR request_id=$6)
               AND ($7::text IS NULL OR category=$7)
             ORDER BY created_at DESC, id DESC
             LIMIT $8",
            &[
                &tenant_id,
                &project_id,
                &actor_scope,
                &filter.work_id,
                &filter.change_id,
                &filter.request_id,
                &filter.category,
                &limit,
            ],
        )
        .await?;
    Ok(rows.iter().map(row_to_record).collect())
}

async fn count_denies(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    actor_scope: Option<&str>,
    filter: &OpsHistoryFilter,
) -> PgResult<i64> {
    let n: i64 = tx
        .query_one(
            "SELECT COUNT(*)::bigint FROM awr_team.ops_audit_denies
             WHERE tenant_id=$1 AND project_id=$2
               AND ($3::text IS NULL OR actor_id=$3)
               AND ($4::text IS NULL OR request_id=$4)",
            &[&tenant_id, &project_id, &actor_scope, &filter.request_id],
        )
        .await?
        .get(0);
    Ok(n)
}

async fn list_denies(
    tx: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    actor_scope: Option<&str>,
    filter: &OpsHistoryFilter,
    limit: i64,
) -> PgResult<Vec<Value>> {
    let rows = tx
        .query(
            "SELECT id, category, action, actor_id, client_id, person_id,
                    target_kind, target_id, request_id, reason_code, created_at
             FROM awr_team.ops_audit_denies
             WHERE tenant_id=$1 AND project_id=$2
               AND ($3::text IS NULL OR actor_id=$3)
               AND ($4::text IS NULL OR request_id=$4)
             ORDER BY created_at DESC, id DESC
             LIMIT $5",
            &[
                &tenant_id,
                &project_id,
                &actor_scope,
                &filter.request_id,
                &limit,
            ],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let created: std::time::SystemTime = row.get("created_at");
        let created_ms = created
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        out.push(json!({
            "id": row.get::<_, String>("id"),
            "category": row.get::<_, String>("category"),
            "action": row.get::<_, String>("action"),
            "actor_id": row.get::<_, Option<String>>("actor_id"),
            "client_id": row.get::<_, Option<String>>("client_id"),
            "person_id": row.get::<_, Option<String>>("person_id"),
            "target_kind": row.get::<_, Option<String>>("target_kind"),
            "target_id": row.get::<_, Option<String>>("target_id"),
            "request_id": row.get::<_, Option<String>>("request_id"),
            "reason_code": row.get::<_, String>("reason_code"),
            "created_at_unix_ms": created_ms,
            "redacted": true
        }));
    }
    Ok(out)
}

/// Convenience builder from authenticated authority.
pub(crate) fn write_from_auth(
    auth: &ReaderAuthority,
    category: OpsCategory,
    action: impl Into<String>,
    target_kind: impl Into<String>,
) -> OpsAuditWrite {
    OpsAuditWrite {
        category,
        action: action.into(),
        result: "committed",
        person_id: None,
        actor_id: auth.actor_id.clone(),
        client_id: auth.client_id.clone(),
        target_kind: target_kind.into(),
        target_id: None,
        work_id: None,
        change_id: None,
        request_id: None,
        membership_version: Some(auth.membership_version),
        authority_version: None,
        policy_version: Some(awr_team::PERMISSION_POLICY_VERSION as i32),
        source_version: if auth.snapshot.is_empty() {
            None
        } else {
            Some(auth.snapshot.clone())
        },
        digest: None,
        summary: json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_drops_secret_and_chat_keys() {
        let v = redact_summary(json!({
            "secret_hash": "abc",
            "action": "planning.approve",
            "chat": "hello",
            "token_usage": 9,
            "digest": "deadbeef"
        }));
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("secret_hash"));
        assert!(!obj.contains_key("chat"));
        assert!(!obj.contains_key("token_usage"));
        assert_eq!(obj.get("action").unwrap(), "planning.approve");
        assert_eq!(obj.get("digest").unwrap(), "deadbeef");
    }

    #[test]
    fn sanitize_reason_redacts_secret_words() {
        assert_eq!(sanitize_reason("bearer leaked"), "redacted_deny");
        assert_eq!(sanitize_reason("permission_denied"), "permission_denied");
    }

    #[test]
    fn digest_is_stable_hex() {
        let d = digest_of(&json!({"a":1}));
        assert_eq!(d.len(), 64);
        assert_eq!(d, digest_of(&json!({"a":1})));
    }
}
