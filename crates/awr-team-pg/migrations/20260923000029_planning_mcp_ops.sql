BEGIN;

-- AWR-TMCP-023: idempotent planning command receipts for HTTP/MCP parity.
-- Mutating planning ops reuse the original request_id receipt on disconnect;
-- callers must not resubmit with a new ID.

CREATE TABLE awr_team.planning_command_receipts (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN (
        'planning.propose',
        'planning.edit_draft',
        'planning.approve',
        'planning.publish',
        'planning.activate'
    )),
    request_hash TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'completed' CHECK (status IN ('reserved', 'completed')),
    result_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, request_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);

ALTER TABLE awr_team.planning_command_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_command_receipts FORCE ROW LEVEL SECURITY;

CREATE POLICY planning_command_receipts_isolation ON awr_team.planning_command_receipts
    USING (tenant_id = current_setting('awr.tenant_id', true)
       AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
       AND project_id = current_setting('awr.project_id', true));

UPDATE awr_team.schema_state SET version=29 WHERE component='awr_team';

COMMIT;
