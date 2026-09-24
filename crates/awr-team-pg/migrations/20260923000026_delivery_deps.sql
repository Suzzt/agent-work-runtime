BEGIN;

-- Versioned cross-stream delivery dependencies and adoption credentials (WS-030).
-- Tenant/project keys match the responsibility-table pattern; historical rows are retained.
CREATE TABLE awr_team.hard_delivery_dependencies (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    provider_work_id TEXT NOT NULL,
    provider_workstream_id TEXT NOT NULL,
    consumer_work_id TEXT NOT NULL,
    consumer_workstream_id TEXT NOT NULL,
    policy TEXT NOT NULL CHECK (policy IN ('fixed_delivery', 'current_contract')),
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    completion_receipt TEXT NOT NULL,
    contract_sha256 TEXT NOT NULL,
    artifact_sha256 TEXT NOT NULL,
    created_at_ms BIGINT NOT NULL,
    revoked_at_ms BIGINT,
    body_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id),
    CHECK (provider_work_id <> consumer_work_id),
    CHECK (provider_workstream_id <> consumer_workstream_id)
);
CREATE INDEX hard_delivery_dependencies_consumer
    ON awr_team.hard_delivery_dependencies(tenant_id, project_id, consumer_work_id, status);
CREATE INDEX hard_delivery_dependencies_provider
    ON awr_team.hard_delivery_dependencies(tenant_id, project_id, provider_work_id, status);

CREATE TABLE awr_team.export_authorizations (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    provider_work_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('granted', 'denied', 'revoked')),
    completion_receipt TEXT NOT NULL,
    contract_sha256 TEXT NOT NULL,
    artifact_sha256 TEXT NOT NULL,
    export_scope_sha256 TEXT NOT NULL,
    granted_by TEXT NOT NULL,
    created_at_ms BIGINT NOT NULL,
    revoked_at_ms BIGINT,
    body_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);
CREATE INDEX export_authorizations_provider
    ON awr_team.export_authorizations(tenant_id, project_id, provider_work_id, status);

CREATE TABLE awr_team.adoption_credentials (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    dependency_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'stale', 'revoked')),
    completion_receipt TEXT NOT NULL,
    export_authorization_id TEXT NOT NULL,
    adopted_at_ms BIGINT NOT NULL,
    body_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id),
    FOREIGN KEY (tenant_id, project_id, dependency_id)
        REFERENCES awr_team.hard_delivery_dependencies(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, export_authorization_id)
        REFERENCES awr_team.export_authorizations(tenant_id, project_id, id)
);
CREATE INDEX adoption_credentials_dependency
    ON awr_team.adoption_credentials(tenant_id, project_id, dependency_id, status);

CREATE TABLE awr_team.delivery_credential_receipts (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN (
        'register_dependency', 'revoke_dependency',
        'grant_export', 'revoke_export', 'adopt'
    )),
    event_id TEXT NOT NULL,
    replayed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at_ms BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, project_id, request_key),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);


-- Tenant/project isolation (FORCE so NOSUPERUSER NOBYPASSRLS app cannot bypass).
DO $$
DECLARE t TEXT;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'hard_delivery_dependencies',
        'export_authorizations',
        'adoption_credentials',
        'delivery_credential_receipts'
    ]
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

UPDATE awr_team.schema_state SET version=26 WHERE component='awr_team';

COMMIT;
