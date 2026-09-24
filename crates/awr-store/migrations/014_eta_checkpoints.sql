-- Calibrated expected acceptance time and stage checkpoints (WS-043).
-- Forecasts are append-only and stored separately from measured usage/time.
-- Historical rows are never updated in place; reestimates supersede by link.
CREATE TABLE eta_forecasts (
    project_id TEXT NOT NULL REFERENCES projects(id),
    forecast_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    generated_at_ms INTEGER NOT NULL CHECK (generated_at_ms >= 0),
    task_graph_version TEXT NOT NULL,
    execution_strategy TEXT NOT NULL,
    sample_policy_id TEXT NOT NULL,
    method_version TEXT NOT NULL,
    estimate_kind TEXT NOT NULL CHECK (estimate_kind IN (
        'provisional', 'unestimable', 'calibrated'
    )),
    supersedes_forecast_id TEXT,
    body_json TEXT NOT NULL CHECK (json_valid(body_json)),
    recorded_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, forecast_id),
    FOREIGN KEY (project_id, supersedes_forecast_id)
        REFERENCES eta_forecasts(project_id, forecast_id)
);
CREATE INDEX eta_forecasts_target
    ON eta_forecasts(project_id, target_id, generated_at_ms);
CREATE INDEX eta_forecasts_work
    ON eta_forecasts(project_id, work_id, generated_at_ms);

CREATE TABLE eta_sample_ledger (
    project_id TEXT NOT NULL REFERENCES projects(id),
    sample_id TEXT NOT NULL,
    work_kind TEXT NOT NULL,
    source_ledger TEXT NOT NULL CHECK (
        source_ledger <> 'acceptance'
        AND source_ledger NOT LIKE 'acceptance:%'
    ),
    observed_execution_ms INTEGER NOT NULL CHECK (observed_execution_ms >= 0),
    observed_wait_ms INTEGER NOT NULL CHECK (observed_wait_ms >= 0),
    body_json TEXT NOT NULL CHECK (json_valid(body_json)),
    recorded_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, sample_id)
);

CREATE TABLE eta_acceptance_data (
    project_id TEXT NOT NULL REFERENCES projects(id),
    acceptance_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    accepted_at_ms INTEGER NOT NULL CHECK (accepted_at_ms >= 0),
    body_json TEXT NOT NULL CHECK (json_valid(body_json)),
    recorded_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, acceptance_id)
);

CREATE TABLE eta_ingest_receipts (
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN (
        'append_forecast', 'record_sample', 'record_acceptance'
    )),
    event_id TEXT NOT NULL,
    replayed INTEGER NOT NULL DEFAULT 0 CHECK (replayed IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, request_key)
);
