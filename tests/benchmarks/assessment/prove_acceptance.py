#!/usr/bin/env python3
"""Prove AWR-DEC-013's three acceptance criteria from frozen artifacts (offline)."""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
CE = ROOT / "tests" / "fixtures" / "assessment" / "counterexamples"


def main() -> int:
    # A1: freeze matrix, fixture identity, expect/forbid, compare scripts; budgets+stats before candidate
    matrix = json.loads((CE / "coverage-matrix.json").read_text())
    manifest = json.loads((CE / "manifest.json").read_text())
    contract = json.loads((HERE / "contract.json").read_text())
    budgets = json.loads((HERE / "budgets.json").read_text())
    assert len(matrix["classes"]) == 20
    assert len(manifest["fixtures"]) == 20
    for name in manifest["fixtures"]:
        body = json.loads((CE / name).read_text())
        assert body["case"].startswith("C")
        assert "expect" in body and "forbidden" in body
    assert (HERE / "compare.py").is_file()
    assert contract["status"].startswith("preregistered_before_first_candidate")
    assert budgets["status"].startswith("frozen_slots_pending_baseline") or budgets[
        "status"
    ].startswith("frozen_with_baseline")
    assert contract["statistics"]["minimum_samples_for_p95"] == 30
    a1 = True

    # A2: families separate; unmeasured not derived
    families = set(contract["metric_families"])
    assert families == {
        "structural_correctness",
        "runtime_overhead",
        "advisory_effectiveness",
    }
    assert set(contract["unmeasured_not_derived"]) == {
        "model_success_rate",
        "dollar_savings",
        "semantic_understanding",
    }
    a2 = True

    # A3: critical families covered; hard fails not offset by averages
    required = {
        "error_retry",
        "revoke",
        "wrong_task",
        "evidence_withdrawal",
        "missing_fields",
        "delivery_unknown",
        "time_budget",
    }
    assert set(manifest["critical_families_zero_tolerance"]) == required
    samples = json.loads((HERE / "sample-receipts.json").read_text())
    sys.path.insert(0, str(HERE))
    import compare as compare_mod

    bad = compare_mod.compare_pair(
        contract,
        budgets,
        samples["baseline_receipt"],
        samples["candidate_receipt_hard_fail_offset_attempt"],
    )
    assert bad["passed"] is False
    assert bad["families"]["structural_correctness"].get("offset_attempt_rejected") is True
    a3 = True

    # nested verifies
    for script in (
        CE / "verify.py",
        HERE / "verify.py",
        ROOT / "tests" / "fixtures" / "assessment" / "envelope" / "verify.py",
    ):
        r = subprocess.run([sys.executable, str(script)], capture_output=True, text=True)
        assert r.returncode == 0, script.name + "\n" + r.stderr + r.stdout

    print(
        json.dumps(
            {
                "work": "AWR-DEC-013",
                "acceptance": {
                    "1_freeze_matrix_identity_expect_forbid_compare_budgets_stats": a1,
                    "2_families_separate_unmeasured_not_derived": a2,
                    "3_critical_hard_fails_not_offset_by_averages": a3,
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
