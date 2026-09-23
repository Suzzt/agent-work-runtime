BEGIN;

-- AWR-TMCP-021: planning suggestions and controlled draft candidates.
-- Suggestions are advisory only (not claimable / not formal work).
-- Candidate history rows are append-only; hard-delete is refused at the API.

CREATE TABLE awr_team.planning_suggestions (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    author_person_id TEXT NOT NULL,
    author_actor_id TEXT NOT NULL,
    author_client_id TEXT NOT NULL,
    rationale TEXT NOT NULL,
    version INT NOT NULL CHECK (version >= 1),
    baseline_digest TEXT NOT NULL,
    baseline_epoch TEXT NOT NULL,
    affected_work_keys JSONB NOT NULL,
    proposed_notes JSONB NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('open', 'accepted_into_draft', 'dismissed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);

CREATE TABLE awr_team.planning_candidates (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    author_person_id TEXT NOT NULL,
    author_actor_id TEXT NOT NULL,
    author_client_id TEXT NOT NULL,
    baseline_digest TEXT NOT NULL,
    baseline_epoch TEXT NOT NULL,
    draft_revision INT NOT NULL CHECK (draft_revision >= 1),
    candidate_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('drafting', 'approved', 'published', 'superseded')),
    changes_json JSONB NOT NULL,
    suggestion_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
    allowed_spec_roots JSONB NOT NULL DEFAULT '[]'::jsonb,
    project_goal_keys JSONB NOT NULL DEFAULT '[]'::jsonb,
    self_approve_policy_json JSONB NOT NULL,
    delivery_completion_policy TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);

-- Append-only history. No UPDATE/DELETE from application paths.
CREATE TABLE awr_team.planning_candidate_history (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    draft_revision INT NOT NULL,
    candidate_digest TEXT NOT NULL,
    changes_json JSONB NOT NULL,
    actor_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, candidate_id, draft_revision),
    FOREIGN KEY (tenant_id, project_id, candidate_id)
        REFERENCES awr_team.planning_candidates(tenant_id, project_id, id)
);

CREATE TABLE awr_team.planning_approvals (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    candidate_digest TEXT NOT NULL,
    draft_revision INT NOT NULL,
    approver_person_id TEXT NOT NULL,
    approver_actor_id TEXT NOT NULL,
    approver_client_id TEXT NOT NULL,
    self_approved BOOLEAN NOT NULL,
    decided_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, candidate_id)
        REFERENCES awr_team.planning_candidates(tenant_id, project_id, id)
);

CREATE TABLE awr_team.planning_publish_receipts (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    candidate_digest TEXT NOT NULL,
    draft_revision INT NOT NULL,
    approval_id TEXT NOT NULL,
    publisher_actor_id TEXT NOT NULL,
    publisher_client_id TEXT NOT NULL,
    -- TMCP-022 consumes this receipt for source writeback; TMCP-021 does not
    -- mutate authoritative source bytes.
    source_writeback_pending BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, candidate_id)
        REFERENCES awr_team.planning_candidates(tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id, approval_id)
        REFERENCES awr_team.planning_approvals(tenant_id, project_id, id)
);

ALTER TABLE awr_team.planning_suggestions ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_suggestions FORCE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_candidates ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_candidates FORCE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_candidate_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_candidate_history FORCE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_approvals ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_approvals FORCE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_publish_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE awr_team.planning_publish_receipts FORCE ROW LEVEL SECURITY;

CREATE POLICY planning_suggestions_isolation ON awr_team.planning_suggestions
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));
CREATE POLICY planning_candidates_isolation ON awr_team.planning_candidates
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));
CREATE POLICY planning_candidate_history_isolation ON awr_team.planning_candidate_history
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));
CREATE POLICY planning_approvals_isolation ON awr_team.planning_approvals
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));
CREATE POLICY planning_publish_receipts_isolation ON awr_team.planning_publish_receipts
    USING (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true))
    WITH CHECK (tenant_id = current_setting('awr.tenant_id', true)
        AND project_id = current_setting('awr.project_id', true));

UPDATE awr_team.schema_state SET version=21 WHERE component='awr_team';
COMMIT;
