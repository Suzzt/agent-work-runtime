BEGIN;

-- Owner-only enabled-project backup/restore operation receipts (AWR-WS-014).
CREATE TABLE awr_team.backup_operations (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    operator_role TEXT NOT NULL,
    op TEXT NOT NULL,
    result_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, request_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);
ALTER TABLE awr_team.backup_operations ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.backup_operations FORCE ROW LEVEL SECURITY;
CREATE POLICY backup_operations_isolation ON awr_team.backup_operations
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

UPDATE awr_team.schema_state SET version=16 WHERE component='awr_team';
COMMIT;
