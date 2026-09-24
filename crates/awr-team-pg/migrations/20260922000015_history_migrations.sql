BEGIN;

-- Owner-only unattributed history migration receipts (AWR-WS-014).
-- Never reachable through application-role client HTTP/MCP.
CREATE TABLE awr_team.history_migrations (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    operator_role TEXT NOT NULL,
    result_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, request_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);
ALTER TABLE awr_team.history_migrations ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.history_migrations FORCE ROW LEVEL SECURITY;
CREATE POLICY history_migrations_isolation ON awr_team.history_migrations
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

UPDATE awr_team.schema_state SET version=15 WHERE component='awr_team';
COMMIT;
