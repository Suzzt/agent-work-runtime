-- Versioned cross-stream delivery dependencies and adoption credentials (WS-030).
-- Hard deps bind exact Work + contract + artifact + completion receipt.
-- Export authorizations and adoption credentials retain auditable history.
CREATE TABLE hard_delivery_dependencies (
    project_id TEXT NOT NULL REFERENCES projects(id),
    id TEXT NOT NULL,
    provider_work_item_id TEXT NOT NULL,
    provider_workstream_id TEXT NOT NULL,
    consumer_work_item_id TEXT NOT NULL,
    consumer_workstream_id TEXT NOT NULL,
    policy TEXT NOT NULL CHECK (policy IN ('fixed_delivery', 'current_contract')),
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
    completion_receipt TEXT NOT NULL,
    contract_sha256 TEXT NOT NULL,
    artifact_sha256 TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    revoked_at_ms INTEGER,
    body_json TEXT NOT NULL,
    PRIMARY KEY (project_id, id),
    CHECK (provider_work_item_id <> consumer_work_item_id),
    CHECK (provider_workstream_id <> consumer_workstream_id)
);
CREATE INDEX hard_delivery_dependencies_consumer
    ON hard_delivery_dependencies(project_id, consumer_work_item_id, status);
CREATE INDEX hard_delivery_dependencies_provider
    ON hard_delivery_dependencies(project_id, provider_work_item_id, status);

CREATE TABLE export_authorizations (
    project_id TEXT NOT NULL REFERENCES projects(id),
    id TEXT NOT NULL,
    provider_work_item_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('granted', 'denied', 'revoked')),
    completion_receipt TEXT NOT NULL,
    contract_sha256 TEXT NOT NULL,
    artifact_sha256 TEXT NOT NULL,
    export_scope_sha256 TEXT NOT NULL,
    granted_by TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    revoked_at_ms INTEGER,
    body_json TEXT NOT NULL,
    PRIMARY KEY (project_id, id)
);
CREATE INDEX export_authorizations_provider
    ON export_authorizations(project_id, provider_work_item_id, status);

CREATE TABLE adoption_credentials (
    project_id TEXT NOT NULL REFERENCES projects(id),
    id TEXT NOT NULL,
    dependency_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'stale', 'revoked')),
    completion_receipt TEXT NOT NULL,
    export_authorization_id TEXT NOT NULL,
    adopted_at_ms INTEGER NOT NULL,
    body_json TEXT NOT NULL,
    PRIMARY KEY (project_id, id),
    FOREIGN KEY (project_id, dependency_id)
        REFERENCES hard_delivery_dependencies(project_id, id),
    FOREIGN KEY (project_id, export_authorization_id)
        REFERENCES export_authorizations(project_id, id)
);
CREATE INDEX adoption_credentials_dependency
    ON adoption_credentials(project_id, dependency_id, status);

CREATE TABLE delivery_credential_receipts (
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN (
        'register_dependency', 'revoke_dependency',
        'grant_export', 'revoke_export', 'adopt'
    )),
    event_id TEXT NOT NULL,
    replayed INTEGER NOT NULL DEFAULT 0 CHECK (replayed IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, request_key)
);
