BEGIN;

-- AWR-TMCP-022: authoritative source writeback + consistent activation receipts.
-- Journals make file+PG dual writes recoverable without exposing mixed live versions.

CREATE TABLE awr_team.planning_writeback_journals (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    candidate_digest TEXT NOT NULL,
    publish_receipt_id TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (phase IN (
        'planned', 'validated', 'source_written', 'pg_activating', 'completed', 'refused', 'rolled_back'
    )),
    before_fingerprint TEXT NOT NULL,
    after_fingerprint TEXT NOT NULL,
    source_version TEXT,
    activated_snapshot_id TEXT,
    authority_epoch TEXT,
    approver_actor_id TEXT,
    publisher_actor_id TEXT NOT NULL,
    affected_work_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
    unrelated_work_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
    recovery_actions JSONB NOT NULL DEFAULT '[]'::jsonb,
    refuse_reason TEXT,
    audit_receipt_id TEXT,
    body_json JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, request_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id),
    FOREIGN KEY (tenant_id, project_id, candidate_id)
        REFERENCES awr_team.planning_candidates(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, publish_receipt_id)
        REFERENCES awr_team.planning_publish_receipts(tenant_id, project_id, id)
);

CREATE TABLE awr_team.planning_activation_receipts (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    candidate_digest TEXT NOT NULL,
    publish_receipt_id TEXT NOT NULL,
    approval_id TEXT NOT NULL,
    approver_actor_id TEXT NOT NULL,
    publisher_actor_id TEXT NOT NULL,
    source_version TEXT NOT NULL,
    activated_snapshot_id TEXT NOT NULL,
    authority_epoch TEXT NOT NULL,
    before_fingerprint TEXT NOT NULL,
    after_fingerprint TEXT NOT NULL,
    affected_work_ids JSONB NOT NULL,
    unrelated_work_ids JSONB NOT NULL,
    audit_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    UNIQUE (tenant_id, project_id, request_id),
    FOREIGN KEY (tenant_id, project_id, candidate_id)
        REFERENCES awr_team.planning_candidates(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, publish_receipt_id)
        REFERENCES awr_team.planning_publish_receipts(tenant_id, project_id, id)
);

ALTER TABLE awr_team.planning_publish_receipts
    ADD COLUMN IF NOT EXISTS activation_receipt_id TEXT,
    ADD COLUMN IF NOT EXISTS source_version TEXT,
    ADD COLUMN IF NOT EXISTS activated_snapshot_id TEXT;

ALTER TABLE awr_team.planning_writeback_journals ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_writeback_journals FORCE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_activation_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_activation_receipts FORCE ROW LEVEL SECURITY;

CREATE POLICY planning_writeback_journals_isolation ON awr_team.planning_writeback_journals
    USING (tenant_id = current_setting('awr.tenant_id', true)
       AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
       AND project_id = current_setting('awr.project_id', true));

CREATE POLICY planning_activation_receipts_isolation ON awr_team.planning_activation_receipts
    USING (tenant_id = current_setting('awr.tenant_id', true)
       AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
       AND project_id = current_setting('awr.project_id', true));

UPDATE awr_team.schema_state SET version=28 WHERE component='awr_team';

COMMIT;
