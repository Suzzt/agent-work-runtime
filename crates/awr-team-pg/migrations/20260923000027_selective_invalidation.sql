BEGIN;

-- Selective invalidation, boundary revalidation, scoped planning changes (WS-032).
-- Historical rows are retained; FORCE RLS matches the responsibility-table pattern.

CREATE TABLE awr_team.selective_invalidation_events (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    provider_work_id TEXT NOT NULL,
    change_kind TEXT NOT NULL CHECK (change_kind IN (
        'new_version_or_progress',
        'current_selection_changed',
        'delivery_revoked',
        'pinned_artifact_unavailable'
    )),
    reevaluate_json JSONB NOT NULL,
    leave_valid_json JSONB NOT NULL,
    unaffected_json JSONB NOT NULL,
    created_at_ms BIGINT NOT NULL,
    body_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);
CREATE INDEX selective_invalidation_events_provider
    ON awr_team.selective_invalidation_events(tenant_id, project_id, provider_work_id);

CREATE TABLE awr_team.execution_boundary_checks (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    execution_id TEXT,
    boundary TEXT NOT NULL CHECK (boundary IN ('prepare', 'dispatch', 'complete')),
    decision TEXT NOT NULL CHECK (decision IN (
        'allow', 'block_new_effects', 'keep_effects_assign_recovery'
    )),
    recovery_duty BOOLEAN NOT NULL,
    erase_effects BOOLEAN NOT NULL DEFAULT FALSE,
    unrelated_may_continue BOOLEAN NOT NULL DEFAULT TRUE,
    reason TEXT NOT NULL,
    created_at_ms BIGINT NOT NULL,
    body_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);
CREATE INDEX execution_boundary_checks_work
    ON awr_team.execution_boundary_checks(tenant_id, project_id, work_id, boundary);

CREATE TABLE awr_team.scoped_planning_changes (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    discovered_by TEXT NOT NULL,
    old_graph_version TEXT NOT NULL,
    new_graph_version TEXT NOT NULL,
    old_acceptance_contract TEXT NOT NULL,
    new_acceptance_contract TEXT NOT NULL,
    affected_json JSONB NOT NULL,
    cancel_split_json JSONB NOT NULL,
    continue_conditions_json JSONB NOT NULL,
    status TEXT NOT NULL CHECK (status IN (
        'recorded', 'affected_blocked', 'confirmed', 'rejected'
    )),
    confirmed_by TEXT,
    created_at_ms BIGINT NOT NULL,
    decided_at_ms BIGINT,
    body_json JSONB NOT NULL,
    PRIMARY KEY (tenant_id, project_id, id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);
CREATE INDEX scoped_planning_changes_status
    ON awr_team.scoped_planning_changes(tenant_id, project_id, status);

CREATE TABLE awr_team.planning_change_action_blocks (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    change_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    PRIMARY KEY (tenant_id, project_id, change_id, work_id),
    FOREIGN KEY (tenant_id, project_id, change_id)
        REFERENCES awr_team.scoped_planning_changes(tenant_id, project_id, id)
);
CREATE INDEX planning_change_action_blocks_work
    ON awr_team.planning_change_action_blocks(tenant_id, project_id, work_id, active);

CREATE TABLE awr_team.selective_invalidation_receipts (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    op TEXT NOT NULL CHECK (op IN (
        'selective_invalidate',
        'boundary_revalidate',
        'record_planning_change',
        'confirm_planning_change',
        'reject_planning_change'
    )),
    event_id TEXT NOT NULL,
    replayed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at_ms BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, project_id, request_key),
    FOREIGN KEY (tenant_id, project_id) REFERENCES awr_team.projects(tenant_id, id)
);

DO $$
DECLARE t TEXT;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'selective_invalidation_events',
        'execution_boundary_checks',
        'scoped_planning_changes',
        'planning_change_action_blocks',
        'selective_invalidation_receipts'
    ]
    LOOP
        EXECUTE format('ALTER TABLE awr_team.%I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE awr_team.%I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY %I_isolation ON awr_team.%I
             USING (tenant_id = current_setting(''awr.tenant_id'', true)
                AND project_id = current_setting(''awr.project_id'', true))
             WITH CHECK (tenant_id = current_setting(''awr.tenant_id'', true)
                AND project_id = current_setting(''awr.project_id'', true))', t, t);
    END LOOP;
END $$;

UPDATE awr_team.schema_state SET version=27 WHERE component='awr_team';

COMMIT;
