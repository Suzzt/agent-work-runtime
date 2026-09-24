-- Agent authorizations: explicit grants with inspect/revoke and narrowed delegation (WS-016).
-- Self-reported skills are stored as hints only; never treated as permission proofs.
CREATE TABLE agent_authorizations (
    project_id TEXT NOT NULL REFERENCES projects(id),
    id TEXT NOT NULL,
    authorizer_person_id TEXT NOT NULL,
    responsible_person_id TEXT NOT NULL,
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('person', 'agent', 'platform_service')),
    subject_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    session_id TEXT,
    model_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'expired')),
    expires_at_ms INTEGER,
    revoked_at_ms INTEGER,
    revoked_by TEXT,
    parent_authorization_id TEXT,
    maintainer_person_id TEXT,
    binding_id TEXT,
    created_at_ms INTEGER NOT NULL,
    body_json TEXT NOT NULL,
    PRIMARY KEY (project_id, id),
    FOREIGN KEY (project_id, authorizer_person_id) REFERENCES persons(project_id, id),
    FOREIGN KEY (project_id, responsible_person_id) REFERENCES persons(project_id, id)
);

CREATE INDEX agent_authorizations_subject
    ON agent_authorizations(project_id, subject_id, status);
CREATE INDEX agent_authorizations_responsible
    ON agent_authorizations(project_id, responsible_person_id, status);

CREATE TABLE agent_authorization_receipts (
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    authorization_id TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN ('issue', 'revoke', 'delegate')),
    event_id TEXT NOT NULL,
    replayed INTEGER NOT NULL DEFAULT 0 CHECK (replayed IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, request_key)
);
