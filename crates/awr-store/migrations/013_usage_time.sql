-- Real usage receipts and measured time observations (WS-041).
-- Costs keep actual / API-equivalent / unknown separate from coverage.
-- Corrections and cross-stream allocations are append-only audit rows.
-- Time intervals retain execution identity so wall-clock ≠ summed duration.
CREATE TABLE usage_receipts (
    project_id TEXT NOT NULL REFERENCES projects(id),
    receipt_id TEXT NOT NULL,
    provider_namespace TEXT NOT NULL,
    provider TEXT NOT NULL,
    call_id TEXT NOT NULL,
    model TEXT NOT NULL,
    session_id TEXT NOT NULL,
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0),
    channel TEXT NOT NULL CHECK (channel IN ('model', 'compaction')),
    work_id TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    occurrence_mainline_id TEXT,
    cost_kind TEXT NOT NULL CHECK (cost_kind IN ('actual', 'api_equivalent_estimate', 'unknown')),
    currency TEXT,
    cost_micros INTEGER CHECK (cost_micros IS NULL OR cost_micros >= 0),
    body_json TEXT NOT NULL CHECK (json_valid(body_json)),
    ingested_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, receipt_id)
);
CREATE UNIQUE INDEX usage_receipts_call
    ON usage_receipts(project_id, provider_namespace, provider, call_id);
CREATE INDEX usage_receipts_execution
    ON usage_receipts(project_id, execution_id, occurred_at_ms);
CREATE INDEX usage_receipts_work
    ON usage_receipts(project_id, work_id, occurred_at_ms);
CREATE INDEX usage_receipts_mainline
    ON usage_receipts(project_id, occurrence_mainline_id, occurred_at_ms);

CREATE TABLE usage_corrections (
    project_id TEXT NOT NULL REFERENCES projects(id),
    correction_id TEXT NOT NULL,
    target_receipt_id TEXT NOT NULL,
    corrected_at_ms INTEGER NOT NULL CHECK (corrected_at_ms >= 0),
    reason TEXT NOT NULL,
    actor TEXT NOT NULL,
    body_json TEXT NOT NULL CHECK (json_valid(body_json)),
    PRIMARY KEY (project_id, correction_id),
    FOREIGN KEY (project_id, target_receipt_id)
        REFERENCES usage_receipts(project_id, receipt_id),
    UNIQUE (project_id, target_receipt_id)
);

CREATE TABLE usage_allocation_records (
    project_id TEXT NOT NULL REFERENCES projects(id),
    allocation_id TEXT NOT NULL,
    receipt_id TEXT NOT NULL,
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0),
    rule TEXT,
    body_json TEXT NOT NULL CHECK (json_valid(body_json)),
    PRIMARY KEY (project_id, allocation_id),
    FOREIGN KEY (project_id, receipt_id)
        REFERENCES usage_receipts(project_id, receipt_id)
);
CREATE UNIQUE INDEX usage_allocation_records_receipt
    ON usage_allocation_records(project_id, receipt_id);

CREATE TABLE usage_counter_snapshots (
    project_id TEXT NOT NULL REFERENCES projects(id),
    provider_namespace TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    session_id TEXT NOT NULL,
    counter_epoch TEXT NOT NULL,
    observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
    body_json TEXT NOT NULL CHECK (json_valid(body_json)),
    PRIMARY KEY (
        project_id, provider_namespace, provider, model, session_id, counter_epoch, observed_at_ms
    )
);

CREATE TABLE usage_execution_intervals (
    project_id TEXT NOT NULL REFERENCES projects(id),
    execution_id TEXT NOT NULL,
    start_ms INTEGER NOT NULL CHECK (start_ms >= 0),
    end_ms INTEGER NOT NULL CHECK (end_ms >= start_ms),
    body_json TEXT NOT NULL CHECK (json_valid(body_json)),
    PRIMARY KEY (project_id, execution_id)
);
CREATE INDEX usage_execution_intervals_range
    ON usage_execution_intervals(project_id, start_ms, end_ms);

CREATE TABLE usage_ingest_receipts (
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN (
        'ingest_receipt', 'record_correction', 'record_allocation',
        'record_counter', 'record_interval'
    )),
    event_id TEXT NOT NULL,
    replayed INTEGER NOT NULL DEFAULT 0 CHECK (replayed IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (project_id, request_key)
);
