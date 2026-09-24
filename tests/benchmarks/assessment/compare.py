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
REQUIRED_COST_KEYS = ("collect_ms", "judge_ms", "output_ms", "return_bytes", "read_ops")
MS_DIMENSIONS = (
    ("collect", "collect_ms"),
    ("judge", "judge_ms"),
    ("output", "output_ms"),
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


def _number(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return float(value)


def _cost_number(costs: Any, key: str) -> float | None:
    if not isinstance(costs, dict) or key not in costs:
        return None
    return _number(costs[key])


def _measured_ms(slot: dict) -> float | None:
    measured = slot.get("measured_baseline")
    if isinstance(measured, dict):
        measured = measured.get("ms")
    return _number(measured)


def runtime_overhead(budgets: dict, baseline: dict, candidate: dict) -> dict:
    """Judge overhead without zero-filling missing costs or unbound budgets.

    A pair is evaluable only after budgets.status is frozen_with_baseline and
    every required cost field is present on both receipts. Missing milliseconds
    are not treated as zero, so they cannot look like an improvement.
    """
    costs_b = baseline.get("costs") if isinstance(baseline.get("costs"), dict) else {}
    costs_c = candidate.get("costs") if isinstance(candidate.get("costs"), dict) else {}
    missing = [
        key
        for key in REQUIRED_COST_KEYS
        if _cost_number(costs_b, key) is None or _cost_number(costs_c, key) is None
    ]
    bound = budgets.get("status") == "frozen_with_baseline"
    overhead: dict[str, Any] = {
        "missing_fields": missing,
        "zero_filled": False,
        "baseline_bound": bound,
        "budgets_status": budgets.get("status"),
        "evaluable": False,
        "tool_call_delta_ok": False,
        "return_bytes_within_ceiling": False,
        "ms_within_ceiling": False,
        "ms_reasons": [],
    }
    if missing:
        return overhead

    tool_delta_max = _number(budgets.get("default_tool_call_delta_max"))
    if tool_delta_max is None:
        tool_delta_max = 0.0
    out_ceil = _number(
        (budgets.get("dimensions") or {}).get("output", {}).get("absolute_ceiling_return_bytes")
    )
    read_delta = costs_c["read_ops"] - costs_b["read_ops"]
    overhead.update(
        {
            "collect_ms_delta": costs_c["collect_ms"] - costs_b["collect_ms"],
            "judge_ms_delta": costs_c["judge_ms"] - costs_b["judge_ms"],
            "output_ms_delta": costs_c["output_ms"] - costs_b["output_ms"],
            "return_bytes_candidate": costs_c["return_bytes"],
            "read_ops_delta": read_delta,
            "tool_call_delta_ok": read_delta <= tool_delta_max,
            "return_bytes_within_ceiling": out_ceil is not None and costs_c["return_bytes"] <= out_ceil,
        }
    )
    if not bound:
        overhead["ms_reasons"] = ["baseline_not_bound"]
        return overhead

    ms_ok = True
    reasons: list[str] = []
    dimensions = budgets.get("dimensions") or {}
    for dim, key in MS_DIMENSIONS:
        slot = dimensions.get(dim) or {}
        measured = _measured_ms(slot)
        absolute = _number(slot.get("absolute_ceiling_ms"))
        factor = _number(slot.get("candidate_ceiling_factor"))
        limit = None
        if measured is not None and factor is not None:
            limit = factor * measured
        if absolute is not None:
            limit = absolute if limit is None else min(limit, absolute)
        if limit is None:
            ms_ok = False
            reasons.append(f"{dim}_baseline_unbound")
            continue
        if costs_c[key] > limit:
            ms_ok = False
            reasons.append(dim)
    overhead["ms_within_ceiling"] = ms_ok
    overhead["ms_reasons"] = reasons
    overhead["evaluable"] = True
    return overhead


def compare_pair(contract: dict, budgets: dict, baseline: dict, candidate: dict) -> dict:
    require(baseline.get("assessment_explain") == "off", "baseline must be explain off")
    require(candidate.get("assessment_explain") == "on", "candidate must be explain on")
    require(candidate.get("role") == "candidate", "candidate role")
    for key in IDENTITY_KEYS:
        require(
            baseline.get(key) is not None and baseline.get(key) == candidate.get(key),
            f"identity mismatch on {key}",
        )

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

    overhead = runtime_overhead(budgets, baseline, candidate)

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

    overhead_ok = (
        overhead["evaluable"]
        and overhead["tool_call_delta_ok"]
        and overhead["return_bytes_within_ceiling"]
        and overhead["ms_within_ceiling"]
        and not overhead["missing_fields"]
    )
    passed = (
        structural["hard_pass"]
        and structural["fixture_pass"]
        and not structural["forbidden_hit"]
        and advisory["match_expect"]
        and overhead_ok
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
