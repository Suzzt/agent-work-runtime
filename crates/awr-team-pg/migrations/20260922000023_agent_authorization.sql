BEGIN;

CREATE TABLE awr_team.agent_authorizations (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    authorizer_person_id TEXT NOT NULL,
    responsible_person_id TEXT NOT NULL,
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('person', 'agent', 'platform_service')),
    subject_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    session_id TEXT,
    model_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'expired')),
    expires_at_ms BIGINT,
    revoked_at_ms BIGINT,
    revoked_by TEXT,
    parent_authorization_id TEXT,
    maintainer_person_id TEXT,
    binding_id TEXT,
    created_at_ms BIGINT NOT NULL,
    body_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id),
    FOREIGN KEY (tenant_id, project_id, authorizer_person_id)
        REFERENCES awr_team.persons(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, responsible_person_id)
        REFERENCES awr_team.persons(tenant_id, project_id, id)
);

CREATE INDEX agent_authorizations_subject
    ON awr_team.agent_authorizations(tenant_id, project_id, subject_id, status);
CREATE INDEX agent_authorizations_responsible
    ON awr_team.agent_authorizations(tenant_id, project_id, responsible_person_id, status);

CREATE TABLE awr_team.agent_authorization_receipts (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    authorization_id TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN ('issue', 'revoke', 'delegate')),
    event_id TEXT NOT NULL,
    replayed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at_ms BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, project_id, request_key)
);

-- Tenant/project isolation. FORCE so a NOSUPERUSER NOBYPASSRLS app role cannot
-- read another tenant's grants by omitting the WHERE clause.
DO $$
DECLARE t TEXT;
BEGIN
    FOREACH t IN ARRAY ARRAY['agent_authorizations', 'agent_authorization_receipts']
    LOOP
        EXECUTE format('ALTER TABLE awr_team.%I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE awr_team.%I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY %I_isolation ON awr_team.%I
             USING (tenant_id = current_setting(''awr.tenant_id'', true)
                AND project_id = current_setting(''awr.project_id'', true))
             WITH CHECK (tenant_id = current_setting(''awr.tenant_id'', true)
                AND project_id = current_setting(''awr.project_id'', true))', t, t);
    END LOOP;
END $$;

UPDATE awr_team.schema_state SET version=23 WHERE component='awr_team';

COMMIT;
