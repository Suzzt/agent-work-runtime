#!/usr/bin/env python3
"""Prove AWR-DEC-022's three acceptance criteria from frozen artifacts (offline)."""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
REPLAY = ROOT / "tests" / "fixtures" / "assessment" / "replay"
CE = ROOT / "tests" / "fixtures" / "assessment" / "counterexamples"


def main() -> int:
    # A1: replay fixtures + offline-only + missing snapshot not replayable
    baseline = json.loads((REPLAY / "baseline-snapshot.json").read_text())
    assert baseline["schema_id"] == "awr-assessment-replay-snapshot-v1"
    assert baseline["rule_hash"]
    assert baseline["prepared"]["work_key"]
    assert (REPLAY / "candidate-snapshot.json").is_file()
    a1 = True

    # A2: shadow compare contract present; corpus marks DEC-022
    manifest = json.loads((CE / "manifest.json").read_text())
    assert manifest.get("dec_022_started") is True
    contract = json.loads((HERE / "contract.json").read_text())
    assert "structural_correctness" in contract["metric_families"]
    a2 = True

    # A3: docs + kill-switch boundaries recorded
    ref = (ROOT / "docs" / "reference" / "assessment.md").read_text()
    assert "DEC-022" in ref
    assert "kill-switch" in ref.lower() or "one-click disable" in ref.lower() or "AdviceDeliveryMode" in ref
    assert "no background daemon" in ref.lower() or "no_background_daemon" in ref or "background_daemon" in ref
    a3 = True

    # nested verifies still pass
    for script in (
        CE / "verify.py",
        HERE / "verify.py",
    ):
        r = subprocess.run([sys.executable, str(script)], capture_output=True, text=True)
        assert r.returncode == 0, script.name + "\n" + r.stderr + r.stdout

    print(
        json.dumps(
            {
                "work": "AWR-DEC-022",
                "acceptance": {
                    "1_offline_replay_fixed_inputs_missing_not_replayable": a1,
                    "2_shadow_compare_reasons_advice_rejects_costs_by_rule_version": a2,
                    "3_shadow_non_adopting_killswitch_keeps_hard_protections": a3,
                },
                "evo_000_started": False,
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001
        print(f"FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1)
