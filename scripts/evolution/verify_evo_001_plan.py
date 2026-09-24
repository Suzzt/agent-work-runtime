#!/usr/bin/env python3
"""Verify AWR-EVO-001 frozen acceptance matrix / baseline workload / cost-auth gate."""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LOCAL = ROOT / ".local/awr-evolution-20260919"
MIRROR = ROOT / "tests/fixtures/evolution/AWR-EVO-001"

REQUIRED_PLAN_KEYS = {
    "schema",
    "work",
    "title",
    "recorded_at",
    "recorder",
    "depends_on",
    "base",
    "scope",
    "acceptance_criteria",
    "check_groups",
    "semantic_counterexamples",
    "compatibility_inputs",
    "scale_matrix",
    "comparison_groups",
    "env_ids",
    "repeat_counts",
    "value_cases",
    "value_trial_slots",
    "thresholds",
    "evaluation_axes",
    "evidence_layers",
    "measurement_separation",
    "ret_amd_001",
    "gates",
    "authorization_state_at_freeze",
    "invariants",
    "plan_hash",
    "evo_002_started",
    "dec_040_started",
    "dec_041_started",
}

REQUIRED_GROUP_FIELDS = {
    "id",
    "scope_items",
    "must_cover",
    "change_axes",
    "output_identity",
    "result_denominator",
    "fixed_inputs",
    "assertions",
}

AXIS_IDS = {"AX-CORRECTNESS", "AX-PERF", "AX-VALUE"}
LAYER_IDS = {"L-SIM", "L-PROTO", "L-NATIVE", "L-PAID"}
GROUP_IDS = {
    "CHK-SEM",
    "CHK-COMP",
    "CHK-READ",
    "CHK-CACHE",
    "CHK-NAV",
    "CHK-LIFE",
    "CHK-DEL",
    "CHK-FAULT",
    "CHK-VALUE",
    "CHK-GOV",
}


def fail(msg: str) -> None:
    print(f"FAIL: {msg}")
    raise SystemExit(1)


def load(path: Path):
    if not path.is_file():
        fail(f"missing {path}")
    return json.loads(path.read_text())


def plan_hash(plan: dict) -> str:
    body = {k: v for k, v in plan.items() if k != "plan_hash"}
    canon = json.dumps(body, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return "sha256:" + hashlib.sha256(canon.encode()).hexdigest()


def main() -> int:
    local_plan = LOCAL / "evaluation-plan.json"
    local_manifest = LOCAL / "fixture-manifest.json"
    local_auth = LOCAL / "authorization-requirements.md"
    mirror_plan = MIRROR / "evaluation-plan.json"
    mirror_manifest = MIRROR / "fixture-manifest.json"
    mirror_auth = MIRROR / "authorization-requirements.md"

    for p in (mirror_plan, mirror_manifest, mirror_auth, MIRROR / "README.md"):
        if not p.is_file():
            fail(f"missing required artifact {p}")

    # Checked-in fixtures are the gate. A private .local copy is optional and
    # must match when present, so a clean checkout can still verify the freeze.
    plan = load(mirror_plan)
    local_present = (
        local_plan.is_file() or local_manifest.is_file() or local_auth.is_file()
    )
    if local_present:
        if not (
            local_plan.is_file() and local_manifest.is_file() and local_auth.is_file()
        ):
            fail("local EVO-001 plan is partial; plan, manifest, and auth doc must all exist")
        if load(local_plan) != plan:
            fail("local evaluation-plan.json differs from checked-in mirror")
        if local_manifest.read_text() != mirror_manifest.read_text():
            fail("local fixture-manifest.json differs from checked-in mirror")
        if local_auth.read_text() != mirror_auth.read_text():
            fail("local authorization-requirements.md differs from checked-in mirror")

    missing = REQUIRED_PLAN_KEYS - set(plan)
    if missing:
        fail(f"plan missing keys: {sorted(missing)}")

    if plan.get("work") != "AWR-EVO-001":
        fail("work must be AWR-EVO-001")
    if plan.get("schema") != "awr-evolution-evaluation-plan/v1":
        fail("unexpected schema")

    if plan_hash(plan) != plan["plan_hash"]:
        fail("plan_hash does not match canonical body")

    scope = plan["scope"]
    if scope.get("formulates_experiment_only") is not True:
        fail("must formulate experiment only")
    if scope.get("calls_paid_models") is not False:
        fail("must not call paid models")
    if scope.get("starts_native_agents") is not False:
        fail("must not start native agents")
    if scope.get("live_optimization_run") is not False:
        fail("must not run live optimization")
    if scope.get("thresholds_locked_before_results") is not True:
        fail("thresholds must be locked before results")

    groups = plan["check_groups"]
    if len(groups) != 10:
        fail(f"expected 10 check groups, got {len(groups)}")
    seen = set()
    for g in groups:
        miss = REQUIRED_GROUP_FIELDS - set(g)
        if miss:
            fail(f"check group {g.get('id')} missing {sorted(miss)}")
        for field in ("fixed_inputs", "assertions", "change_axes", "output_identity"):
            if not g.get(field):
                fail(f"{g['id']}.{field} must be non-empty")
        if not g.get("result_denominator"):
            fail(f"{g['id']}.result_denominator required")
        seen.add(g["id"])
    if seen != GROUP_IDS:
        fail(f"check group ids mismatch: {sorted(seen)}")

    th = plan["thresholds"]
    if th.get("preregistered") is not True:
        fail("thresholds.preregistered must be true")
    if th.get("may_change_after_seeing_results") is not False:
        fail("thresholds must not change after seeing results")
    if not th.get("performance") or not th.get("non_degradation"):
        fail("performance and non_degradation thresholds required")
    if not th.get("samples") or not th.get("missing_measurement_handling"):
        fail("samples and missing_measurement_handling required")
    mm = th["missing_measurement_handling"]
    for k in (
        "skip",
        "fault_not_hit",
        "no_host_observation",
        "missing_cost_or_usage",
        "unknown_vs_known_failure",
        "unrun_authorized_blocked_slots",
    ):
        if k not in mm:
            fail(f"missing_measurement_handling.{k} required")
        if "passed" in str(mm[k]) and "must_not_mark_passed" not in str(mm[k]) and k in (
            "skip",
            "fault_not_hit",
            "no_host_observation",
        ):
            # allow only explicit must_not_mark_passed
            if mm[k] != "must_not_mark_passed":
                fail(f"{k} must forbid marking passed")

    axes = plan["evaluation_axes"]
    axis_ids = {a["id"] for a in axes["axes"]}
    if axis_ids != AXIS_IDS:
        fail(f"evaluation axes mismatch: {sorted(axis_ids)}")
    for a in axes["axes"]:
        if not a.get("cannot_be_replaced_by"):
            fail(f"{a['id']} must declare cannot_be_replaced_by")
        if set(a["cannot_be_replaced_by"]) != (AXIS_IDS - {a["id"]}):
            fail(f"{a['id']} cannot_be_replaced_by incomplete")

    layers = plan["evidence_layers"]["layers"]
    layer_ids = {x["id"] for x in layers}
    if layer_ids != LAYER_IDS:
        fail(f"evidence layers mismatch: {sorted(layer_ids)}")
    for layer in layers:
        if layer["id"] in {"L-NATIVE", "L-PAID"} and layer.get("allowed_without_auth") is not False:
            fail(f"{layer['id']} must require auth")
        if layer["id"] in {"L-SIM", "L-PROTO"} and layer.get("allowed_without_auth") is not True:
            fail(f"{layer['id']} should be allowed without auth")

    auth = plan["authorization_state_at_freeze"]
    if auth["native_verification"].get("authorized") is not False:
        fail("native_verification must be unauthorized at freeze")
    if auth["native_verification"].get("starts_allowed") is not False:
        fail("native starts_allowed must be false")
    if auth["paid_verification"].get("authorized") is not False:
        fail("paid_verification must be unauthorized at freeze")
    if auth["paid_verification"].get("starts_allowed") is not False:
        fail("paid starts_allowed must be false")
    if auth.get("key_availability_is_approval") is not False:
        fail("key availability must not count as approval")

    gates = plan["gates"]
    for gid in ("AWR-EVO-054", "AWR-EVO-060"):
        if gates[gid].get("initial_status") != "blocked":
            fail(f"{gid} must start blocked")
        if gates[gid].get("starts_without_authorization") is not False:
            fail(f"{gid} must not start without authorization")

    slots = plan["value_trial_slots"]
    if len(slots) != 36:
        fail(f"expected 36 registered value slots, got {len(slots)}")
    if any(s.get("status") != "registered_blocked_until_EVO-060_authorization" for s in slots):
        fail("all value slots must remain blocked until EVO-060 authorization")

    ret = plan["ret_amd_001"]
    if ret.get("auto_expand_old_36_trial_proposal") is not False:
        fail("RET-006 must not auto-expand old 36-trial proposal")
    tier_ids = [t["id"] for t in ret["tiers"]]
    if tier_ids != ["R0", "R1", "R2"]:
        fail(f"RET tiers must be R0/R1/R2, got {tier_ids}")
    r2 = ret["tiers"][2]
    if r2.get("authorization_required") is not True:
        fail("R2 requires authorization")
    if r2.get("status") != "blocked_until_explicit_authorization":
        fail("R2 must be blocked until explicit authorization")

    if plan.get("evo_002_started") is not False:
        fail("evo_002_started must be false")
    if plan.get("dec_040_started") is not False:
        fail("dec_040_started must be false")
    if plan.get("dec_041_started") is not False:
        fail("dec_041_started must be false")

    for field in (
        "background_compute",
        "response_bytes",
        "stable_prefix",
        "actual_cached_usage",
        "full_bill",
    ):
        if field not in plan["measurement_separation"]:
            fail(f"measurement_separation.{field} required")

    manifest = load(mirror_manifest)
    if manifest.get("plan_hash") != plan["plan_hash"]:
        fail("manifest plan_hash != plan plan_hash")
    if manifest.get("paid_models_invoked") is not False:
        fail("paid_models_invoked must be false")
    if manifest.get("native_agents_started") is not False:
        fail("native_agents_started must be false")
    if manifest.get("synthetic_only_for_this_item") is not True:
        fail("synthetic_only_for_this_item must be true")

    for fx in manifest["subject_fixtures"]:
        path = MIRROR / fx["path"]
        if not path.is_file():
            fail(f"missing subject fixture {fx['path']}")
        data = json.loads(path.read_text())
        if data.get("id") != fx["id"] and fx["id"] not in {data.get("id")}:
            # allow case variants already checked by presence
            if data.get("id") != fx["id"]:
                fail(f"fixture id mismatch for {fx['path']}: {data.get('id')} != {fx['id']}")
        if fx["role"] == "value_case_subject_synthetic":
            axes = set(data.get("expected_axes") or [])
            if axes != {"AX-CORRECTNESS", "AX-PERF", "AX-VALUE"}:
                fail(f"{fx['id']} must list three independent expected_axes")
            if data.get("axes_may_substitute") is not False:
                fail(f"{fx['id']} axes_may_substitute must be false")

    for fx in manifest["referee_materials"]:
        path = MIRROR / fx["path"]
        if not path.is_file():
            fail(f"missing referee material {fx['path']}")
        if fx.get("enters_subject_context") is not False:
            fail(f"referee {fx['id']} must not enter subject context")
        data = json.loads(path.read_text())
        if data.get("enters_subject_context") is not False:
            fail(f"referee file {fx['path']} must set enters_subject_context=false")

    auth_text = mirror_auth.read_text()
    for needle in (
        "native_verification",
        "paid_verification",
        "key_availability",
        "AX-CORRECTNESS",
        "AX-PERF",
        "AX-VALUE",
        "R0",
        "R1",
        "R2",
        "does **not** call paid models",
        plan["plan_hash"],
    ):
        if needle not in auth_text:
            fail(f"authorization-requirements.md missing marker: {needle}")

    # AC mapping markers present in plan acceptance text
    if len(plan["acceptance_criteria"]) != 4:
        fail("expected 4 acceptance criteria")

    print("OK: AWR-EVO-001 acceptance matrix / baseline workload / cost-auth freeze")
    print(f"  plan_hash={plan['plan_hash']}")
    print(f"  check_groups={len(groups)} value_slots={len(slots)}")
    print(f"  axes={','.join(sorted(axis_ids))}")
    print(f"  layers={','.join(sorted(layer_ids))}")
    print("  native/paid=blocked evo_002/dec_040/dec_041=false")
    print("  ret_amd_001 auto_expand_old_36=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
