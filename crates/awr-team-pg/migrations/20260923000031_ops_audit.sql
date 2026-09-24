BEGIN;

-- AWR-TMCP-040: permission / planning / delivery operations audit.
-- Success records are written in the same business transaction as events/receipts.
-- Deny records use a separate capacity-bounded channel that never mutates business state.
-- This is operational audit only: not full chat text, arbitrary tool I/O, or token billing.
-- PG audit does NOT claim resistance to database-owner tampering or enterprise non-repudiation.

CREATE TABLE awr_team.ops_audit_records (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('access', 'planning', 'delivery')),
    action TEXT NOT NULL,
    result TEXT NOT NULL CHECK (result IN ('committed', 'succeeded')),
    person_id TEXT,
    actor_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN (
        'work', 'change', 'member', 'request', 'candidate', 'delivery',
        'access_plan', 'suggestion', 'review', 'completion'
    )),
    target_id TEXT,
    work_id TEXT,
    change_id TEXT,
    request_id TEXT,
    membership_version BIGINT,
    authority_version BIGINT,
    policy_version INTEGER,
    source_version TEXT,
    digest TEXT,
    summary_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);

CREATE INDEX ops_audit_records_work
    ON awr_team.ops_audit_records (tenant_id, project_id, work_id, created_at);
CREATE INDEX ops_audit_records_change
    ON awr_team.ops_audit_records (tenant_id, project_id, change_id, created_at);
CREATE INDEX ops_audit_records_actor
    ON awr_team.ops_audit_records (tenant_id, project_id, actor_id, created_at);
CREATE INDEX ops_audit_records_request
    ON awr_team.ops_audit_records (tenant_id, project_id, request_id)
    WHERE request_id IS NOT NULL;
CREATE INDEX ops_audit_records_created
    ON awr_team.ops_audit_records (tenant_id, project_id, created_at DESC);

ALTER TABLE awr_team.ops_audit_records ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.ops_audit_records FORCE ROW LEVEL SECURITY;
CREATE POLICY ops_audit_records_isolation ON awr_team.ops_audit_records
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

-- Capacity-bounded deny channel. Application prunes oldest rows per project.
CREATE TABLE awr_team.ops_audit_denies (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    category TEXT NOT NULL CHECK (category IN ('access', 'planning', 'delivery', 'other')),
    action TEXT NOT NULL,
    actor_id TEXT,
    client_id TEXT,
    person_id TEXT,
    target_kind TEXT,
    target_id TEXT,
    request_id TEXT,
    reason_code TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);

CREATE INDEX ops_audit_denies_created
    ON awr_team.ops_audit_denies (tenant_id, project_id, created_at DESC);
CREATE INDEX ops_audit_denies_actor
    ON awr_team.ops_audit_denies (tenant_id, project_id, actor_id, created_at);

ALTER TABLE awr_team.ops_audit_denies ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.ops_audit_denies FORCE ROW LEVEL SECURITY;
CREATE POLICY ops_audit_denies_isolation ON awr_team.ops_audit_denies
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

UPDATE awr_team.schema_state SET version = 31 WHERE component = 'awr_team';

COMMIT;
