use awr_core::*;
use std::path::PathBuf;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/workstreams/migration-takeover/project-snapshot.json")
}

fn load_fixture() -> MigrationFixture {
    let raw = std::fs::read_to_string(fixture_path()).expect("fixture present");
    serde_json::from_str(&raw).expect("fixture deserializes as MigrationFixture")
}

#[test]
fn independent_fixture_backup_preview_migrate_restore() {
    let fixture = load_fixture();
    let receipt = run_drill(&fixture).expect("drill");
    assert!(receipt.ok, "{receipt:?}");
    assert!(!receipt.identity_forged);
    assert!(!receipt.agent_auto_promoted);
    assert!(receipt.restore_matches_backup);
    for line in ["Team", "EVO", "DEC", "AUTO"] {
        assert!(
            receipt.goal_lines_preserved.iter().any(|l| l == line),
            "missing {line} in {:?}",
            receipt.goal_lines_preserved
        );
    }
    assert!(receipt.preview_pending_confirmation >= 1);
}

#[test]
fn independent_fixture_marks_unprovable_and_blocks_agent_approver_promotion() {
    let fixture = load_fixture();
    let backed = backup(&fixture).unwrap();
    let prev = preview(&backed).unwrap();
    assert!(prev.safe_to_migrate, "{:?}", prev.refusals);

    let orphan = prev
        .history
        .iter()
        .find(|h| h.kind == "session" && h.id == "sess-orphan")
        .expect("orphan agent session");
    assert_eq!(orphan.disposition, HistoryDisposition::PendingConfirmation);
    assert!(!orphan.person_delegation_proven);

    let agent_approver = prev
        .history
        .iter()
        .find(|h| h.kind == "review" && h.id == "rev-agent-as-approver")
        .expect("agent-as-approver review");
    assert_eq!(
        agent_approver.disposition,
        HistoryDisposition::PendingConfirmation
    );
    assert!(!agent_approver.promoted_to_owner_or_approver);
    assert!(
        prev.history
            .iter()
            .all(|h| !h.promoted_to_owner_or_approver)
    );
    assert!(!prev.incomplete_work_item_ids.is_empty());
    assert!(!prev.active_session_ids.is_empty());
    assert!(!prev.release_evidence_ids.is_empty());
}

#[test]
fn project_takeover_dry_run_follows_proven_fixture_path() {
    let fixture = load_fixture();
    let drill = run_drill(&fixture).unwrap();
    assert!(drill.ok);
    let backed = backup(&fixture).unwrap();
    let prev = preview(&backed).unwrap();
    let ready = project_takeover_preview(&ProjectTakeoverInput {
        project_id: "01M1YJBR5PW6QXYGABADJVAJPC".into(),
        goals: fixture.goals.clone(),
        history: prev.history.clone(),
        incomplete_work_item_ids: prev.incomplete_work_item_ids.clone(),
        active_session_ids: prev.active_session_ids.clone(),
        release_evidence_ids: prev.release_evidence_ids.clone(),
        fixture_drill_ok: true,
        fixture_backup_digest: drill.backup_digest.clone(),
    });
    assert!(ready.ok, "{:?}", ready.refusals);
    assert_eq!(ready.mode, "dry_run_ready");
    assert!(!ready.identity_forged);
    assert!(!ready.agent_auto_promoted);
    assert!(ready.incomplete_preserved);
    assert!(ready.active_sessions_preserved);
    assert!(ready.release_evidence_preserved);

    let blocked = project_takeover_preview(&ProjectTakeoverInput {
        project_id: "01M1YJBR5PW6QXYGABADJVAJPC".into(),
        goals: fixture.goals.clone(),
        history: prev.history,
        incomplete_work_item_ids: prev.incomplete_work_item_ids,
        active_session_ids: prev.active_session_ids,
        release_evidence_ids: prev.release_evidence_ids,
        fixture_drill_ok: false,
        fixture_backup_digest: drill.backup_digest,
    });
    assert!(!blocked.ok);
    assert!(
        blocked
            .refusals
            .iter()
            .any(|r| r == "independent_fixture_drill_required_first")
    );
}
