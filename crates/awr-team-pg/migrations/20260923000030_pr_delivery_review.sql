BEGIN;

-- TMCP-031: independent review.decide grant + PR delivery version evidence.
-- review.decide is never implied by role templates; admin cannot skip acceptance.
-- PR GitHub facts are manually registered with fact_source + observed_at (no webhook).

ALTER TABLE awr_team.project_memberships
    ADD COLUMN IF NOT EXISTS independent_review BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE awr_team.pr_deliveries (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    contract_hash TEXT NOT NULL,
    repository TEXT NOT NULL,
    pr_number INTEGER NOT NULL CHECK (pr_number > 0),
    pr_url TEXT NOT NULL,
    head_sha TEXT NOT NULL CHECK (head_sha ~ '^[0-9a-f]{40}$'),
    merge_sha TEXT CHECK (merge_sha IS NULL OR merge_sha ~ '^[0-9a-f]{40}$'),
    test_evidence_id TEXT,
    test_evidence_digest TEXT,
    -- GitHub observation flags (separate from AWR review/acceptance).
    gh_submitted BOOLEAN NOT NULL DEFAULT TRUE,
    gh_approved BOOLEAN NOT NULL DEFAULT FALSE,
    gh_merged BOOLEAN NOT NULL DEFAULT FALSE,
    fact_source TEXT NOT NULL CHECK (fact_source IN (
        'authorized_human_github_verification',
        'operator_recorded_observation'
    )),
    observed_at TEXT NOT NULL,
    registered_by_actor_id TEXT NOT NULL,
    author_actor_id TEXT,
    owner_person_id TEXT,
    executor_actor_id TEXT,
    state TEXT NOT NULL CHECK (state IN ('active', 'invalidated')),
    invalidation_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, work_id)
        REFERENCES awr_team.work_items(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, test_evidence_id)
        REFERENCES awr_team.evidence(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, registered_by_actor_id)
        REFERENCES awr_team.actors(tenant_id, id)
);

CREATE INDEX pr_deliveries_work_active
    ON awr_team.pr_deliveries(tenant_id, project_id, work_id, state);

ALTER TABLE awr_team.pr_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.pr_deliveries FORCE ROW LEVEL SECURITY;
CREATE POLICY pr_deliveries_isolation ON awr_team.pr_deliveries
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

-- Attribution columns on completion receipts (author/owner/executor/reviewer/final-submitter).
ALTER TABLE awr_team.completion_receipts
    ADD COLUMN IF NOT EXISTS author_actor_id TEXT,
    ADD COLUMN IF NOT EXISTS owner_person_id TEXT,
    ADD COLUMN IF NOT EXISTS executor_actor_id TEXT,
    ADD COLUMN IF NOT EXISTS final_submitter_actor_id TEXT,
    ADD COLUMN IF NOT EXISTS pr_delivery_id TEXT;

ALTER TABLE awr_team.completion_receipts
    ADD CONSTRAINT completion_receipts_pr_delivery_fk
    FOREIGN KEY (tenant_id, project_id, pr_delivery_id)
    REFERENCES awr_team.pr_deliveries(tenant_id, project_id, id);

UPDATE awr_team.schema_state SET version = 30 WHERE component = 'awr_team';

COMMIT;
