BEGIN;

-- TMCP-012: project-admin MCP member/role/credential receipts use the app role.
-- Owner-only awr_team.access_changes remains revoked from the application role.
CREATE TABLE awr_team.project_access_changes (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    admin_actor_id TEXT NOT NULL,
    admin_client_id TEXT NOT NULL,
    result_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, request_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);
ALTER TABLE awr_team.project_access_changes ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.project_access_changes FORCE ROW LEVEL SECURITY;
CREATE POLICY project_access_changes_isolation ON awr_team.project_access_changes
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

-- Accept TMCP-010 template names alongside legacy membership labels.
ALTER TABLE awr_team.project_memberships
    DROP CONSTRAINT IF EXISTS project_memberships_role_check;
ALTER TABLE awr_team.project_memberships
    ADD CONSTRAINT project_memberships_role_check
    CHECK (role IN (
        'admin', 'worker', 'reviewer', 'reader',
        'project_admin', 'developer', 'maintainer'
    ));

UPDATE awr_team.schema_state SET version=20 WHERE component='awr_team';
COMMIT;
