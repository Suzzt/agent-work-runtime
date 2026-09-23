CREATE TABLE team_handoffs (
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    work_item_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('execution', 'responsibility')),
    status TEXT NOT NULL CHECK (status IN (
        'proposed', 'inspected', 'accepted', 'rejected', 'cancelled', 'timed_out'
    )),
    version INTEGER NOT NULL CHECK (version >= 1),
    from_person_id TEXT NOT NULL,
    to_person_id TEXT NOT NULL,
    body_json TEXT NOT NULL,
    accept_request_key TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, id)
);

CREATE INDEX team_handoffs_work_status
    ON team_handoffs(project_id, work_item_id, status);

CREATE UNIQUE INDEX team_handoffs_one_open_per_work
    ON team_handoffs(project_id, work_item_id)
    WHERE status IN ('proposed', 'inspected');

CREATE TABLE team_handoff_receipts (
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    handoff_id TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN (
        'propose', 'inspect', 'accept', 'reject', 'cancel', 'timeout'
    )),
    event_id TEXT NOT NULL,
    replayed INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, request_key)
);
