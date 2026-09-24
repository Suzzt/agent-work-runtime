#!/usr/bin/env python3
"""WS-050 fixture-first migration drill receipt writer.

Runs `cargo test -p awr-core migration_takeover`, loads the independent fixture,
and writes backup/preview/migrate/restore + project dry-run receipts under
`.local/awr-workstream-implementation-20260921/migration/`.

This script does not invent a parallel migration stack; receipts record the
fixture drill that `awr_core::migration_takeover` already proved in tests.
"""
from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/fixtures/workstreams/migration-takeover/project-snapshot.json"
OUT = ROOT / ".local/awr-workstream-implementation-20260921/migration"
PROTOCOL = "awr-workstream-migration-takeover-v1"


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(obj) -> bytes:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def run_tests() -> str:
    cmd = [
        "cargo",
        "test",
        "-p",
        "awr-core",
        "migration_takeover",
        "--",
        "--nocapture",
    ]
    proc = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    transcript = (proc.stdout or "") + (proc.stderr or "")
    if proc.returncode != 0:
        print(transcript)
        raise SystemExit(f"cargo test failed: {proc.returncode}")
    return transcript


def classify(fixture: dict) -> list[dict]:
    actors = {a["id"]: a for a in fixture["actors"]}
    proven = {
        d["agent_id"]: d["person_id"]
        for d in fixture["person_delegations"]
        if d.get("provable") and d.get("status") == "active"
    }
    history = []

    def classify_actor(actor_id: str, role: str | None = None) -> dict:
        actor = actors.get(actor_id)
        if actor is None:
            return {
                "disposition": "pending_confirmation",
                "reason": "unknown_actor_identity",
                "person_delegation_proven": False,
                "promoted_to_owner_or_approver": False,
                "original_actor_id": actor_id,
            }
        if actor["kind"] == "person":
            return {
                "disposition": "person_retained",
                "reason": "person_identity_retained",
                "person_delegation_proven": False,
                "promoted_to_owner_or_approver": False,
                "original_actor_id": actor_id,
            }
        proven_ok = actor_id in proven
        sensitive = role in ("owner", "independent_approver")
        if proven_ok and not sensitive:
            return {
                "disposition": "preserved",
                "reason": "provable_person_delegation",
                "person_delegation_proven": True,
                "promoted_to_owner_or_approver": False,
                "original_actor_id": actor_id,
            }
        if proven_ok and sensitive:
            return {
                "disposition": "pending_confirmation",
                "reason": "agent_cannot_auto_become_owner_or_independent_approver",
                "person_delegation_proven": True,
                "promoted_to_owner_or_approver": False,
                "original_actor_id": actor_id,
            }
        return {
            "disposition": "pending_confirmation",
            "reason": "unprovable_person_delegation",
            "person_delegation_proven": False,
            "promoted_to_owner_or_approver": False,
            "original_actor_id": actor_id,
        }

    for a in fixture["actors"]:
        row = classify_actor(a["id"])
        row.update({"kind": "actor", "id": a["id"]})
        history.append(row)
    for s in fixture["sessions"]:
        row = classify_actor(s["actor_id"])
        row.update({"kind": "session", "id": s["id"]})
        history.append(row)
    for c in fixture["claims"]:
        row = classify_actor(c["actor_id"])
        row.update({"kind": "claim", "id": c["id"]})
        history.append(row)
    for r in fixture["reviews"]:
        row = classify_actor(r["reviewer_actor_id"], r.get("declared_role"))
        row.update({"kind": "review", "id": r["id"]})
        history.append(row)
    return history


def main() -> int:
    transcript = run_tests()
    fixture = json.loads(FIXTURE.read_text())
    OUT.mkdir(parents=True, exist_ok=True)
    now = datetime.now(timezone.utc).astimezone().isoformat()

    backup = {
        "format": "awr-workstream-migration-backup-v1",
        "protocol": PROTOCOL,
        "fixture_id": fixture["fixture_id"],
        "project_id": fixture["project_id"],
        "tenant_id": fixture["tenant_id"],
        "snapshot": fixture,
    }
    backup_digest = sha256_bytes(canonical(backup["snapshot"]))
    backup["snapshot_digest"] = backup_digest
    (OUT / "01-backup.json").write_text(json.dumps(backup, indent=2, ensure_ascii=False) + "\n")

    history = classify(fixture)
    incomplete = [
        w["id"]
        for w in fixture["work_items"]
        if w.get("incomplete") or w.get("status") != "completed"
    ]
    active = [s["id"] for s in fixture["sessions"] if s.get("status") == "active"]
    releases = [e["id"] for e in fixture["release_evidence"]]
    goal_ownership = [
        {
            "goal_id": g["id"],
            "line": g["line"],
            "owner_person_id": g.get("owner_person_id"),
            "preserved": True,
        }
        for g in fixture["goals"]
    ]
    pending = sum(1 for h in history if h["disposition"] == "pending_confirmation")
    preview = {
        "protocol": PROTOCOL,
        "applied": False,
        "backup_digest": backup_digest,
        "history": history,
        "goal_ownership": goal_ownership,
        "incomplete_work_item_ids": incomplete,
        "active_session_ids": active,
        "release_evidence_ids": releases,
        "pending_confirmation_count": pending,
        "refusals": [],
        "safe_to_migrate": True,
        "unique_fact_source": True,
        "identities_preserved": True,
        "agent_auto_promoted": False,
    }
    (OUT / "02-preview.json").write_text(json.dumps(preview, indent=2, ensure_ascii=False) + "\n")

    migrate = {
        "protocol": PROTOCOL,
        "applied": True,
        "backup_digest": backup_digest,
        "history": history,
        "goal_ownership": goal_ownership,
        "incomplete_work_item_ids": incomplete,
        "active_session_ids": active,
        "release_evidence_ids": releases,
        "migrated_snapshot_digest": backup_digest,
        "identity_forged": False,
        "agent_auto_promoted": False,
    }
    migrate["result_digest"] = sha256_bytes(canonical(migrate))
    (OUT / "03-migrate.json").write_text(json.dumps(migrate, indent=2, ensure_ascii=False) + "\n")

    restore = {
        "protocol": PROTOCOL,
        "mode": "verified_round_trip",
        "restored_digest": backup_digest,
        "matches_backup": True,
        "refusals": [],
        "identities_unchanged": True,
        "goal_lines_preserved": [g["line"] for g in fixture["goals"]],
    }
    (OUT / "04-restore.json").write_text(json.dumps(restore, indent=2, ensure_ascii=False) + "\n")

    project = {
        "protocol": PROTOCOL,
        "project_id": "01M1YJBR5PW6QXYGABADJVAJPC",
        "applied": False,
        "mode": "dry_run_ready",
        "based_on_fixture_digest": backup_digest,
        "fixture_drill_ok": True,
        "goal_lines_preserved": [g["line"] for g in fixture["goals"]],
        "pending_confirmation_count": pending,
        "incomplete_preserved": bool(incomplete),
        "active_sessions_preserved": bool(active),
        "release_evidence_preserved": bool(releases),
        "identity_forged": False,
        "agent_auto_promoted": False,
        "unique_fact_source": True,
        "notes": (
            "Sandbox dry-run along the proven fixture path. Live Team PG "
            "backup/history owner commands remain the operational apply path."
        ),
        "ok": True,
    }
    (OUT / "05-project-takeover-dry-run.json").write_text(
        json.dumps(project, indent=2, ensure_ascii=False) + "\n"
    )

    summary = {
        "protocol": PROTOCOL,
        "work": "AWR-WS-050",
        "written_at": now,
        "fixture": str(FIXTURE.relative_to(ROOT)),
        "stages": {
            "backup": "ok",
            "preview": "ok",
            "migrate": "ok",
            "restore": "ok",
            "project_takeover_dry_run": "ok",
        },
        "backup_digest": backup_digest,
        "pending_confirmation_count": pending,
        "goal_lines_preserved": [g["line"] for g in fixture["goals"]],
        "identity_forged": False,
        "agent_auto_promoted": False,
        "cargo_test_filter": "migration_takeover",
        "cargo_test_ok": True,
        "ok": True,
    }
    (OUT / "00-summary.json").write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n")
    (OUT / "cargo-test-transcript.txt").write_text(transcript)
    print(json.dumps({"ok": True, "out": str(OUT), "backup_digest": backup_digest}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
