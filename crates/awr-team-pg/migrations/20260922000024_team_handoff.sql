BEGIN;

-- Confirmed Team long-term handoff (WS-017).
-- Execution handoff and responsibility transfer are separate kinds.
CREATE TABLE awr_team.team_handoffs (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('execution', 'responsibility')),
    status TEXT NOT NULL CHECK (status IN (
        'proposed', 'inspected', 'accepted', 'rejected', 'cancelled', 'timed_out'
    )),
    version BIGINT NOT NULL CHECK (version >= 1),
    from_person_id TEXT NOT NULL,
    to_person_id TEXT NOT NULL,
    package_json JSONB NOT NULL,
    proposed_successor_json JSONB,
    accepted_successor_json JSONB,
    proposer_execution_id TEXT,
    proposer_fence BIGINT,
    expires_at_ms BIGINT,
    created_at_ms BIGINT NOT NULL,
    updated_at_ms BIGINT NOT NULL,
    inspected_at_ms BIGINT,
    terminal_at_ms BIGINT,
    terminal_reason TEXT,
    accept_request_key TEXT,
    body_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id),
    FOREIGN KEY (tenant_id, project_id, from_person_id)
        REFERENCES awr_team.persons(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, to_person_id)
        REFERENCES awr_team.persons(tenant_id, project_id, id)
);

CREATE INDEX team_handoffs_work_status
    ON awr_team.team_handoffs(tenant_id, project_id, work_id, status);

-- At most one open handoff per work item (propose→inspect window).
CREATE UNIQUE INDEX team_handoffs_one_open_per_work
    ON awr_team.team_handoffs(tenant_id, project_id, work_id)
    WHERE status IN ('proposed', 'inspected');

CREATE TABLE awr_team.team_handoff_receipts (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    handoff_id TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN (
        'propose', 'inspect', 'accept', 'reject', 'cancel', 'timeout'
    )),
    event_id TEXT NOT NULL,
    replayed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at_ms BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, project_id, request_key),
    FOREIGN KEY (tenant_id, project_id, handoff_id)
        REFERENCES awr_team.team_handoffs(tenant_id, project_id, id)
);

UPDATE awr_team.schema_state SET version=24 WHERE component='awr_team';

COMMIT;
