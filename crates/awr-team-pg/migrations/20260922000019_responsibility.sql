BEGIN;

-- Explicit person responsibility units. Never infer person↔agent from actors.kind.
CREATE TABLE awr_team.persons (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled', 'departed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);

CREATE TABLE awr_team.person_agent_bindings (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    person_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, person_id)
        REFERENCES awr_team.persons(tenant_id, project_id, id),
    UNIQUE (tenant_id, project_id, person_id, agent_id)
);

-- Task roles: sole owner, collaborators (side table), current executor, independent reviewer.
-- Temporary execution claim must not rewrite ownership. Unassigned pool permits NULL owner.
CREATE TABLE awr_team.task_responsibilities (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    owner_person_id TEXT,
    independent_reviewer_person_id TEXT,
    executor_kind TEXT CHECK (executor_kind IN ('person', 'agent_run') OR executor_kind IS NULL),
    executor_person_id TEXT,
    executor_agent_id TEXT,
    executor_binding_id TEXT,
    version BIGINT NOT NULL CHECK (version >= 0),
    pending_kind TEXT CHECK (
        pending_kind IN ('departure', 'disabled', 'no_acceptor', 'legacy_identity_migration')
        OR pending_kind IS NULL
    ),
    pending_person_id TEXT,
    pending_legacy_ref TEXT,
    pending_transfer_request_key TEXT,
    pending_detail TEXT,
    personal_mode_default BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, work_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id),
    FOREIGN KEY (tenant_id, project_id, owner_person_id)
        REFERENCES awr_team.persons(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, independent_reviewer_person_id)
        REFERENCES awr_team.persons(tenant_id, project_id, id),
    CHECK (
        (executor_kind IS NULL AND executor_person_id IS NULL AND executor_agent_id IS NULL AND executor_binding_id IS NULL)
        OR (executor_kind = 'person' AND executor_person_id IS NOT NULL AND executor_agent_id IS NULL AND executor_binding_id IS NULL)
        OR (executor_kind = 'agent_run' AND executor_person_id IS NOT NULL AND executor_agent_id IS NOT NULL AND executor_binding_id IS NOT NULL)
    ),
    CHECK (
        (pending_kind IS NULL AND pending_detail IS NULL)
        OR (pending_kind IS NOT NULL AND pending_detail IS NOT NULL AND length(pending_detail) > 0)
    )
);

CREATE TABLE awr_team.task_collaborators (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    person_id TEXT NOT NULL,
    PRIMARY KEY (tenant_id, project_id, work_id, person_id),
    FOREIGN KEY (tenant_id, project_id, work_id)
        REFERENCES awr_team.task_responsibilities(tenant_id, project_id, work_id),
    FOREIGN KEY (tenant_id, project_id, person_id)
        REFERENCES awr_team.persons(tenant_id, project_id, id)
);

CREATE TABLE awr_team.responsibility_events (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    version_before BIGINT NOT NULL,
    version_after BIGINT NOT NULL,
    actor_person_id TEXT,
    payload_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id)
);
CREATE INDEX responsibility_events_task
    ON awr_team.responsibility_events(tenant_id, project_id, work_id, created_at);

CREATE TABLE awr_team.responsibility_receipts (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    event_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    op TEXT NOT NULL,
    version_before BIGINT NOT NULL,
    version_after BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, request_key),
    FOREIGN KEY (tenant_id, project_id, event_id)
        REFERENCES awr_team.responsibility_events(tenant_id, project_id, id)
);

-- Tenant/project isolation (FORCE so NOSUPERUSER NOBYPASSRLS app cannot bypass).
DO $$
DECLARE t TEXT;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'persons',
        'person_agent_bindings',
        'task_responsibilities',
        'task_collaborators',
        'responsibility_events',
        'responsibility_receipts'
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

UPDATE awr_team.schema_state SET version=19 WHERE component='awr_team';
COMMIT;
