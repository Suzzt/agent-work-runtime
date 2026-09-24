#!/usr/bin/env python3
"""Verify DEC-013 compare contract, budgets freeze, and compare harness behavior."""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def load(name: str):
    return json.loads((HERE / name).read_text())


def main() -> int:
    contract = load("contract.json")
    budgets = load("budgets.json")
    samples = load("sample-receipts.json")

    assert contract["work"] == 'AWR-DEC-013'
    assert contract["status"] == 'preregistered_before_first_candidate_run'
    assert contract["method"]["count_separately_from_evo"] is True
    assert contract["method"]["evo_paid_or_dual_host_required"] is False
    assert set(contract["method"]["costs_recorded"]) == set(('collect', 'judge', 'output'))
    assert set(contract["metric_families"]) == set(('structural_correctness', 'runtime_overhead', 'advisory_effectiveness'))
    assert set(contract["unmeasured_not_derived"]) == set(('model_success_rate', 'dollar_savings', 'semantic_understanding'))
    assert contract["statistics"]["minimum_samples_for_p95"] == 30
    assert contract["statistics"]["include_timeouts_failures_overruns_in_denominator"] is True
    assert set(contract["hard_constraint_policy"]["critical_families_zero_tolerance"]) == set(('error_retry', 'revoke', 'wrong_task', 'evidence_withdrawal', 'missing_fields', 'delivery_unknown', 'time_budget'))

    assert budgets["status"] == 'frozen_slots_pending_baseline_binding'
    assert "fabricate_ms_improvement" in budgets["forbid"]
    for dim in ("collect", "judge", "output"):
        assert dim in budgets["dimensions"]

    sys.path.insert(0, str(HERE))
    import compare as compare_mod

    try:
        compare_mod.nearest_rank_p95([float(i) for i in range(29)])
        raise AssertionError("p95 with N<30 should fail")
    except ValueError:
        pass
    assert compare_mod.nearest_rank_p95([float(i) for i in range(1, 31)]) == 29.0

    pending = compare_mod.compare_pair(
        contract, budgets, samples["baseline_receipt"], samples["candidate_receipt_ok"]
    )
    assert pending["passed"] is False
    assert pending["families"]["runtime_overhead"]["baseline_bound"] is False
    assert pending["families"]["runtime_overhead"]["zero_filled"] is False
    assert pending["families_kept_separate"] is True

    bound = json.loads(json.dumps(budgets))
    bound["status"] = "frozen_with_baseline"
    bound["dimensions"]["collect"]["measured_baseline"] = 10.0
    bound["dimensions"]["judge"]["measured_baseline"] = 1.0
    bound["dimensions"]["output"]["measured_baseline"] = 2.0
    ok = compare_mod.compare_pair(
        contract, bound, samples["baseline_receipt"], samples["candidate_receipt_ok"]
    )
    assert ok["passed"] is True
    assert ok["families"]["runtime_overhead"]["evaluable"] is True

    stripped = json.loads(json.dumps(samples["candidate_receipt_ok"]))
    del stripped["costs"]["collect_ms"]
    missing = compare_mod.compare_pair(
        contract, bound, samples["baseline_receipt"], stripped
    )
    assert missing["passed"] is False
    assert "collect_ms" in missing["families"]["runtime_overhead"]["missing_fields"]
    assert "collect_ms_delta" not in missing["families"]["runtime_overhead"]

    bad = compare_mod.compare_pair(
        contract,
        budgets,
        samples["baseline_receipt"],
        samples["candidate_receipt_hard_fail_offset_attempt"],
    )
    assert bad["passed"] is False
    assert bad["families"]["structural_correctness"]["hard_pass"] is False
    assert bad["families"]["structural_correctness"].get("offset_attempt_rejected") is True

    with tempfile.TemporaryDirectory() as td:
        td_path = Path(td)
        base = td_path / "baseline.json"
        cand_ok = td_path / "candidate_ok.json"
        cand_bad = td_path / "candidate_bad.json"
        base.write_text(json.dumps(samples["baseline_receipt"]))
        cand_ok.write_text(json.dumps(samples["candidate_receipt_ok"]))
        cand_bad.write_text(json.dumps(samples["candidate_receipt_hard_fail_offset_attempt"]))
        bound_path = td_path / "budgets.json"
        bound_path.write_text(json.dumps(bound))
        r_pending = subprocess.run(
            [sys.executable, str(HERE / "compare.py"), "--baseline", str(base), "--candidate", str(cand_ok)],
            capture_output=True,
            text=True,
            check=False,
        )
        assert r_pending.returncode == 1, r_pending.stdout
        r1 = subprocess.run(
            [
                sys.executable,
                str(HERE / "compare.py"),
                "--budgets",
                str(bound_path),
                "--baseline",
                str(base),
                "--candidate",
                str(cand_ok),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        assert r1.returncode == 0, r1.stderr + r1.stdout
        r2 = subprocess.run(
            [sys.executable, str(HERE / "compare.py"), "--baseline", str(base), "--candidate", str(cand_bad)],
            capture_output=True,
            text=True,
            check=False,
        )
        assert r2.returncode == 1, r2.stdout

    doc = ROOT / "docs" / "benchmarks" / "assessment.md"
    assert doc.is_file(), "docs/benchmarks/assessment.md missing"
    text = doc.read_text()
    assert "structural_correctness" in text
    assert "model_success_rate" in text or "模型成功率" in text
    assert "baseline" in text and "candidate" in text

    print("DEC-013 assessment benchmark contract OK")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001
        print(f"FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1)
