CREATE TABLE operation_readset_receipts (
    project_id TEXT NOT NULL REFERENCES projects(id),
    request_id TEXT NOT NULL,
    identity_hash TEXT NOT NULL,
    readset_json TEXT NOT NULL CHECK(json_valid(readset_json)),
    event_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, request_id),
    UNIQUE(project_id, event_id),
    FOREIGN KEY(project_id, event_id) REFERENCES events(project_id, id)
) STRICT;
