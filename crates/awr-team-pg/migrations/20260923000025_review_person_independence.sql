-- WS-018: person-level review independence and completion audit columns.
-- Same human with two agents is not independent team acceptance.
-- Reject/return/rework keep history; invalidated rounds are retained.

ALTER TABLE awr_team.review_rounds
    ADD COLUMN author_person_id TEXT,
    ADD COLUMN evidence_id TEXT,
    ADD COLUMN execution_id TEXT,
    ADD COLUMN artifact_digest TEXT,
    ADD COLUMN execution_result_digest TEXT;

ALTER TABLE awr_team.review_rounds
    ADD CONSTRAINT review_rounds_author_person_fk
    FOREIGN KEY (tenant_id, project_id, author_person_id)
    REFERENCES awr_team.persons(tenant_id, project_id, id);

ALTER TABLE awr_team.review_rounds
    ADD CONSTRAINT review_rounds_evidence_fk
    FOREIGN KEY (tenant_id, project_id, evidence_id)
    REFERENCES awr_team.evidence(tenant_id, project_id, id);

ALTER TABLE awr_team.review_decisions
    ADD COLUMN reviewer_person_id TEXT,
    ADD COLUMN independence_kind TEXT NOT NULL DEFAULT 'unspecified'
        CHECK (independence_kind IN (
            'team_independent',
            'personal_self_review',
            'unspecified'
        ));

ALTER TABLE awr_team.review_decisions
    ADD CONSTRAINT review_decisions_reviewer_person_fk
    FOREIGN KEY (tenant_id, project_id, reviewer_person_id)
    REFERENCES awr_team.persons(tenant_id, project_id, id);

ALTER TABLE awr_team.completion_receipts
    ADD COLUMN independence_kind TEXT
        CHECK (independence_kind IS NULL OR independence_kind IN (
            'team_independent',
            'personal_self_review',
            'ordinary_confirm',
            'unspecified'
        )),
    ADD COLUMN evidence_id TEXT,
    ADD COLUMN execution_id TEXT,
    ADD COLUMN approved_by_person_id TEXT,
    ADD COLUMN submitted_by_person_id TEXT;

ALTER TABLE awr_team.completion_receipts
    ADD CONSTRAINT completion_receipts_evidence_fk
    FOREIGN KEY (tenant_id, project_id, evidence_id)
    REFERENCES awr_team.evidence(tenant_id, project_id, id);

UPDATE awr_team.schema_state SET version = 25 WHERE component = 'awr_team';
