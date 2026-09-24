#!/usr/bin/env python3
"""Verify AWR-EVO-002 Team-compatible planning validator fixtures and gate."""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MIRROR = ROOT / "tests/fixtures/evolution/AWR-EVO-002"
LOCAL = ROOT / ".local/awr-evolution-20260919/plan-validator-compat"
PY = ROOT / ".venv/bin/python"
if not PY.is_file():
    PY = Path(sys.executable)

VALIDATOR = ROOT / "scripts/evolution/plan_validator_compat.py"


def fail(msg: str) -> None:
    print(f"FAIL: {msg}")
    raise SystemExit(1)


def run(args: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(args, cwd=ROOT, text=True, capture_output=True)


def expect_fail(fixture: Path, needle: str) -> str:
    proc = run([str(PY), str(VALIDATOR), "--fixture", str(fixture)])
    out = (proc.stdout + proc.stderr).strip()
    if proc.returncode == 0:
        fail(f"expected FAIL for {fixture}: got PASS\n{out}")
    if needle not in out:
        fail(f"expected needle {needle!r} in FAIL output for {fixture}:\n{out}")
    return out


def expect_pass(fixture: Path) -> str:
    proc = run([str(PY), str(VALIDATOR), "--fixture", str(fixture)])
    out = (proc.stdout + proc.stderr).strip()
    if proc.returncode != 0:
        fail(f"expected PASS for {fixture}:\n{out}")
    if "PASS:" not in out:
        fail(f"missing PASS marker for {fixture}:\n{out}")
    return out


def main() -> int:
    required = [
        MIRROR / "README.md",
        MIRROR / "contracts/awr-v1.json",
        MIRROR / "contracts/awr-team-v1.json",
        MIRROR / "contracts/awr-team-acceptance-v1.json",
        MIRROR / "repro/team-extension-no-work-items/ledger.yaml",
        MIRROR / "repro/misplaced-goal-awr-g-team/ledger.yaml",
        MIRROR / "positive/full-book-team-compat/ledger.yaml",
        MIRROR / "positive/verified-team-completion/ledger.yaml",
        MIRROR / "negative/wrong-binding-acceptance-as-work/ledger.yaml",
        MIRROR / "negative/unknown-version/ledger.yaml",
        MIRROR / "negative/unknown-type/ledger.yaml",
        MIRROR / "negative/duplicate-scope/ledger.yaml",
        MIRROR / "negative/forged-completion-v1/ledger.yaml",
        MIRROR / "negative/forged-completion-team-acceptance/ledger.yaml",
        VALIDATOR,
        ROOT / "scripts/evolution/verify_evo_002_plan_validator.py",
        ROOT / "docs/benchmarks/evolution-plan-validator-compat.md",
    ]
    for path in required:
        if not path.is_file():
            fail(f"missing required artifact {path}")

    # AC1: fixed repro of historical FAIL: 'work_items'
    naive = run(
        [
            str(PY),
            str(VALIDATOR),
            "--fixture",
            str(MIRROR / "repro/team-extension-no-work-items"),
            "--reproduce-naive",
        ]
    )
    naive_out = (naive.stdout + naive.stderr).strip()
    if "FAIL: 'work_items'" not in naive_out:
        fail(f"naive repro did not surface FAIL: 'work_items'\n{naive_out}")
    fixed_repro = expect_pass(MIRROR / "repro/team-extension-no-work-items")

    # Misplaced AWR-G-TEAM attributed, not auto-migrated
    goal_out = expect_fail(
        MIRROR / "repro/misplaced-goal-awr-g-team",
        "misplaced_goal_in_work_items: AWR-G-TEAM",
    )

    # AC2: contract types validated separately; negatives rejected
    expect_pass(MIRROR / "positive/full-book-team-compat")
    expect_pass(MIRROR / "positive/verified-team-completion")
    expect_fail(MIRROR / "negative/wrong-binding-acceptance-as-work", "wrong_binding")
    expect_fail(MIRROR / "negative/unknown-version", "unknown contract type/version")
    expect_fail(MIRROR / "negative/unknown-type", "unknown contract type/version")
    expect_fail(MIRROR / "negative/duplicate-scope", "duplicate")
    expect_fail(MIRROR / "negative/forged-completion-v1", "forged completion")
    expect_fail(
        MIRROR / "negative/forged-completion-team-acceptance", "forged completion"
    )

    # AC3: denominators preserved in contract snapshot; EVO-002 stats independent
    team_acceptance = json.loads(
        (MIRROR / "contracts/awr-team-acceptance-v1.json").read_text()
    )
    preserved = team_acceptance.get("preserved_team_v1_denominators") or {}
    if preserved.get("required") != 69 or preserved.get("real_agent_accepted") != 42:
        fail("Team V1 denominators in fixture contract were rewritten")
    public_matrix = json.loads(
        (ROOT / "docs/reference/team-v1-evidence-matrix.json").read_text()
    )
    counts = public_matrix.get("counts") or {}
    if counts.get("required") != 69 or counts.get("real_agent_accepted") != 42:
        fail("public Team V1 denominators changed — EVO-002 must not rewrite them")

    # AC4: full-book local planning check — report raw result honestly
    full = run([str(PY), str(VALIDATOR), "--root", str(ROOT)])
    full_out = (full.stdout + full.stderr).strip()
    if full.returncode == 0:
        fail(
            "full-book unexpectedly PASS on this box; Mac ledger should be absent — "
            f"refusing to claim full-book pass.\n{full_out}"
        )
    if "FAIL:" not in full_out:
        fail(f"full-book did not report raw FAIL:\n{full_out}")

    LOCAL.mkdir(parents=True, exist_ok=True)
    summary = {
        "work": "AWR-EVO-002",
        "schema": "awr-evo-002-plan-validator-compat/v1",
        "naive_repro": naive_out,
        "fixed_repro_pass": True,
        "fixed_repro_excerpt": fixed_repro.splitlines()[0],
        "misplaced_goal_attribution": goal_out.splitlines()[0],
        "full_book_raw": full_out,
        "full_book_pass": False,
        "team_active_work_ownership_parallel_fix": False,
        "team_active_work_ownership_existing": {
            "commit": "ece9d3a8ba20aed7d0dad36b99a6f871b151cb6b",
            "note": (
                "WS-014 digest-gated rebuild-from-manifest ownership slice — "
                "reused/verified as existing; no parallel fix opened"
            ),
        },
        "evo_010_started": False,
        "dec_040_started": False,
        "dec_041_started": False,
        "preserved_team_v1_denominators": preserved,
        "public_team_v1_counts": {
            "required": counts.get("required"),
            "real_agent_accepted": counts.get("real_agent_accepted"),
        },
    }
    (LOCAL / "compat-summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n"
    )
    (LOCAL / "full-book-raw.txt").write_text(full_out + "\n")
    (LOCAL / "naive-repro-raw.txt").write_text(naive_out + "\n")

    print("OK: AWR-EVO-002 plan-validator compatibility gate")
    print(f"  naive_repro={naive_out}")
    print(f"  full_book_raw={full_out}")
    print("  positives=2 negatives=6 repros=2 denominators_preserved=true")
    print("  evo_010/dec_040/dec_041=false parallel_ownership_fix=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
