#!/usr/bin/env python3
"""Compare baseline (explain off) vs candidate (readonly explain on) under DEC-013.

Does not invoke models or networks. Refuses to derive unmeasured success/$/semantics.
Hard-constraint failures cannot be cleared by soft-score averages.
"""
from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
UNMEASURED = ("model_success_rate", "dollar_savings", "semantic_understanding")
IDENTITY_KEYS = (
    "source_sha",
    "fixture_id",
    "policy_id",
    "policy_version",
    "as_of",
    "collector_version",
)


def load_json(path: Path) -> Any:
    return json.loads(path.read_text())


def nearest_rank_p95(samples: list[float]) -> float:
    if len(samples) < 30:
        raise ValueError("refusing to label p95 with N<30")
    ordered = sorted(samples)
    idx = math.ceil(0.95 * len(ordered)) - 1
    return ordered[idx]


def require(cond: bool, msg: str) -> None:
    if not cond:
        raise AssertionError(msg)


def compare_pair(contract: dict, budgets: dict, baseline: dict, candidate: dict) -> dict:
    require(baseline.get("assessment_explain") == "off", "baseline must be explain off")
    require(candidate.get("assessment_explain") == "on", "candidate must be explain on")
    require(candidate.get("role") == "candidate", "candidate role")
    for key in IDENTITY_KEYS:
        require(baseline.get(key) == candidate.get(key), f"identity mismatch on {key}")

    hard_fails = list(candidate.get("structural", {}).get("hard_constraint_failures") or [])
    claimed_avg = bool(candidate.get("structural", {}).get("claimed_pass_via_average"))
    structural = {
        "fixture_pass": bool(candidate.get("structural", {}).get("fixture_pass")),
        "forbidden_hit": bool(candidate.get("structural", {}).get("forbidden_hit")),
        "hard_constraint_failures": hard_fails,
        "hard_pass": len(hard_fails) == 0 and not claimed_avg,
    }
    if claimed_avg and hard_fails:
        structural["hard_pass"] = False
        structural["offset_attempt_rejected"] = True

    costs_b = baseline.get("costs") or {}
    costs_c = candidate.get("costs") or {}
    tool_delta_max = budgets.get("default_tool_call_delta_max", 0)
    overhead = {
        "collect_ms_delta": costs_c.get("collect_ms", 0) - costs_b.get("collect_ms", 0),
        "judge_ms_delta": costs_c.get("judge_ms", 0) - costs_b.get("judge_ms", 0),
        "output_ms_delta": costs_c.get("output_ms", 0) - costs_b.get("output_ms", 0),
        "return_bytes_candidate": costs_c.get("return_bytes"),
        "read_ops_delta": costs_c.get("read_ops", 0) - costs_b.get("read_ops", 0),
        "budgets_status": budgets.get("status"),
        "tool_call_delta_ok": (costs_c.get("read_ops", 0) - costs_b.get("read_ops", 0))
        <= tool_delta_max,
    }
    out_ceil = (
        budgets.get("dimensions", {}).get("output", {}).get("absolute_ceiling_return_bytes")
    )
    if out_ceil is not None and costs_c.get("return_bytes") is not None:
        overhead["return_bytes_within_ceiling"] = costs_c["return_bytes"] <= out_ceil
    else:
        overhead["return_bytes_within_ceiling"] = True

    advisory = {
        "match_expect": bool(candidate.get("advisory", {}).get("match_expect")),
        "codes": list(candidate.get("advisory", {}).get("codes") or []),
    }

    derived_attempts = [
        key
        for key in UNMEASURED
        if key in candidate or key in (candidate.get("derived") or {})
    ]
    families_separate = candidate.get("merged_single_score") is None

    passed = (
        structural["hard_pass"]
        and structural["fixture_pass"]
        and not structural["forbidden_hit"]
        and advisory["match_expect"]
        and overhead["tool_call_delta_ok"]
        and overhead["return_bytes_within_ceiling"]
        and not derived_attempts
        and families_separate
    )

    return {
        "contract_id": contract["contract_id"],
        "version": contract["version"],
        "passed": passed,
        "families": {
            "structural_correctness": structural,
            "runtime_overhead": overhead,
            "advisory_effectiveness": advisory,
        },
        "unmeasured_not_derived": list(UNMEASURED),
        "derived_attempts_rejected": derived_attempts,
        "families_kept_separate": families_separate,
        "budgets_status": budgets.get("status"),
        "notes": [
            "p95 labeling requires N>=30 per statistics contract",
            "hard failures are not offset by averages",
            "EVO experiments counted separately; not required here",
        ],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--contract", type=Path, default=HERE / "contract.json")
    parser.add_argument("--budgets", type=Path, default=HERE / "budgets.json")
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, help="optional JSON report path")
    args = parser.parse_args(argv)

    contract = load_json(args.contract)
    budgets = load_json(args.budgets)
    baseline = load_json(args.baseline)
    candidate = load_json(args.candidate)

    if "baseline_receipt" in baseline:
        baseline = baseline["baseline_receipt"]
    if "candidate_receipt_ok" in candidate and "structural" not in candidate:
        raise SystemExit("pass a single candidate receipt JSON, not the sample bundle")

    report = compare_pair(contract, budgets, baseline, candidate)
    text = json.dumps(report, indent=2)
    if args.output:
        args.output.write_text(text + "\n")
    print(text)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001
        print(f"FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1)
