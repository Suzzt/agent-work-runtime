-- Person responsibility, explicit agent bindings, and versioned task roles.
-- Temporary execution claims do not steal ownership; coordination claims stay separate.
CREATE TABLE persons (
    project_id TEXT NOT NULL REFERENCES projects(id),
    id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled', 'departed')),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, id)
);

CREATE TABLE person_agent_bindings (
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    person_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, id),
    FOREIGN KEY (project_id, person_id) REFERENCES persons(project_id, id),
    UNIQUE (project_id, person_id, agent_id)
);

CREATE TABLE task_responsibilities (
    project_id TEXT NOT NULL REFERENCES projects(id),
    work_item_id TEXT NOT NULL,
    owner_person_id TEXT,
    independent_reviewer_person_id TEXT,
    executor_kind TEXT CHECK (executor_kind IN ('person', 'agent_run') OR executor_kind IS NULL),
    executor_person_id TEXT,
    executor_agent_id TEXT,
    executor_binding_id TEXT,
    version INTEGER NOT NULL CHECK (version >= 0),
    pending_kind TEXT CHECK (
        pending_kind IN ('departure', 'disabled', 'no_acceptor', 'legacy_identity_migration')
        OR pending_kind IS NULL
    ),
    pending_person_id TEXT,
    pending_legacy_ref TEXT,
    pending_transfer_request_key TEXT,
    pending_detail TEXT,
    personal_mode_default INTEGER NOT NULL DEFAULT 0 CHECK (personal_mode_default IN (0, 1)),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, work_item_id),
    FOREIGN KEY (project_id, owner_person_id) REFERENCES persons(project_id, id),
    FOREIGN KEY (project_id, independent_reviewer_person_id) REFERENCES persons(project_id, id),
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

CREATE TABLE task_collaborators (
    project_id TEXT NOT NULL,
    work_item_id TEXT NOT NULL,
    person_id TEXT NOT NULL,
    PRIMARY KEY (project_id, work_item_id, person_id),
    FOREIGN KEY (project_id, work_item_id) REFERENCES task_responsibilities(project_id, work_item_id),
    FOREIGN KEY (project_id, person_id) REFERENCES persons(project_id, id)
);

CREATE TABLE responsibility_events (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    work_item_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    version_before INTEGER NOT NULL,
    version_after INTEGER NOT NULL,
    actor_person_id TEXT,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX responsibility_events_task
    ON responsibility_events(project_id, work_item_id, created_at);

CREATE TABLE responsibility_receipts (
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    event_id TEXT NOT NULL REFERENCES responsibility_events(id),
    work_item_id TEXT NOT NULL,
    op TEXT NOT NULL,
    version_before INTEGER NOT NULL,
    version_after INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, request_key)
);
