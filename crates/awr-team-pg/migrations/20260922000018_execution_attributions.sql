BEGIN;

-- Owner-only explicit execution attribution receipts (AWR-WS-014).
-- Never reachable through application-role client HTTP/MCP.
-- Attribution requires reviewed executor_client_id; never invented.
CREATE TABLE awr_team.execution_attributions (
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
ALTER TABLE awr_team.execution_attributions ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.execution_attributions FORCE ROW LEVEL SECURITY;
CREATE POLICY execution_attributions_isolation ON awr_team.execution_attributions
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

UPDATE awr_team.schema_state SET version=18 WHERE component='awr_team';
COMMIT;
