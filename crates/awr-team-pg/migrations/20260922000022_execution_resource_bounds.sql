BEGIN;

-- WS-021: unify execution resource kinds, worktree vs shared domains, and
-- lease-generation/fence binding so handoff/expiry/recovery refuse stale tokens
-- while unknown effects keep reservations.

ALTER TABLE awr_team.resource_reservations
    DROP CONSTRAINT IF EXISTS resource_reservations_resource_kind_check;

ALTER TABLE awr_team.resource_reservations
    ADD COLUMN IF NOT EXISTS worktree_id TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS lease_generation BIGINT NOT NULL DEFAULT 0
        CHECK (lease_generation >= 0),
    ADD COLUMN IF NOT EXISTS fence BIGINT NOT NULL DEFAULT 0
        CHECK (fence >= 0);

ALTER TABLE awr_team.resource_reservations
    ADD CONSTRAINT resource_reservations_resource_kind_check
    CHECK (resource_kind IN (
        'file', 'dir', 'prefix', 'workspace', 'external', 'integration', 'named'
    ));

-- Shared kinds are project-global; they must not pretend to be worktree-local.
-- Path/workspace kinds may carry an empty worktree_id (legacy project default).
ALTER TABLE awr_team.resource_reservations
    ADD CONSTRAINT resource_reservations_domain_consistency CHECK (
        (resource_kind IN ('external', 'integration', 'named') AND worktree_id = '')
        OR (resource_kind IN ('file', 'dir', 'prefix', 'workspace'))
    );

CREATE INDEX IF NOT EXISTS resource_reservations_domain_lookup
    ON awr_team.resource_reservations (
        tenant_id, project_id, state, resource_kind, worktree_id, canonical_key
    );

UPDATE awr_team.schema_state SET version = 22 WHERE component = 'awr_team';
COMMIT;
