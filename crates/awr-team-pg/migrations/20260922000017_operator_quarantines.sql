BEGIN;

-- Owner-only active-claim / execution quarantine and attribution receipts (AWR-WS-014).
-- Never reachable through application-role client HTTP/MCP.
CREATE TABLE awr_team.operator_quarantines (
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
ALTER TABLE awr_team.operator_quarantines ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.operator_quarantines FORCE ROW LEVEL SECURITY;
CREATE POLICY operator_quarantines_isolation ON awr_team.operator_quarantines
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

UPDATE awr_team.schema_state SET version=17 WHERE component='awr_team';
COMMIT;
