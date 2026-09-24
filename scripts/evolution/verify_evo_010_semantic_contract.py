#!/usr/bin/env python3
"""Verify AWR-EVO-010 five-layer semantics + two host-integration contracts."""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LOCAL = ROOT / ".local/awr-evolution-20260919"
MIRROR = ROOT / "tests/fixtures/evolution/AWR-EVO-010"
DOCS = [
    ROOT / "docs/reference/host-contract.md",
    ROOT / "docs/reference/cli-mcp-contract.md",
    ROOT / "docs/benchmarks/evolution-semantic-contract.md",
]

REQUIRED_LAYER_IDS = [
    "work_readiness",
    "execution_admission",
    "context_completeness",
    "delivery_observation",
    "completion_validity",
]
REQUIRED_HOST_IDS = ["runtime_delegated", "component_only"]
LAYER_CX = {
    "work_readiness": "CX-WR-01",
    "execution_admission": "CX-EA-01",
    "context_completeness": "CX-CC-01",
    "delivery_observation": "CX-DO-01",
    "completion_validity": "CX-CV-01",
}
HOST_CX = {
    "runtime_delegated": "CX-HM-RD-01",
    "component_only": "CX-HM-CO-01",
}
EXTRA_CX = ["CX-SEP-01", "CX-SEP-02", "CX-MIX-01", "CX-COMPAT-01"]


def fail(msg: str) -> None:
    print(f"FAIL: {msg}")
    raise SystemExit(1)


def load(path: Path):
    if not path.is_file():
        fail(f"missing {path}")
    return json.loads(path.read_text())


def require_keys(obj: dict, keys: set[str], label: str) -> None:
    missing = keys - set(obj)
    if missing:
        fail(f"{label} missing keys: {sorted(missing)}")


def main() -> int:
    local_matrix = LOCAL / "semantic-contract-matrix.json"
    mirror_matrix = MIRROR / "semantic-contract-matrix.json"
    for path in [mirror_matrix, MIRROR / "README.md", *DOCS]:
        if not path.is_file():
            fail(f"missing required artifact {path}")

    # Checked-in fixture is the gate. A private .local copy is optional and
    # must match when present, so a clean checkout can verify the freeze.
    if local_matrix.is_file() and local_matrix.read_text() != mirror_matrix.read_text():
        fail("local semantic-contract-matrix.json differs from checked-in mirror")

    matrix = load(mirror_matrix)
    require_keys(
        matrix,
        {
            "schema",
            "schema_version",
            "work",
            "contract_version",
            "premises",
            "layers",
            "host_modes",
            "ownership_matrix",
            "separations",
            "field_source_audit",
            "compatibility",
            "counterexamples",
            "acceptance_mapping",
            "gates",
            "invariants",
            "matrix_hash",
        },
        "matrix",
    )

    if matrix.get("work") != "AWR-EVO-010":
        fail(f"unexpected work id {matrix.get('work')}")
    if matrix.get("schema") != "awr-evolution-semantic-contract-matrix/v1":
        fail(f"unexpected schema {matrix.get('schema')}")

    layers = matrix["layers"]
    if len(layers) != 5:
        fail(f"expected 5 layers, got {len(layers)}")
    layer_ids = [layer["id"] for layer in layers]
    if layer_ids != REQUIRED_LAYER_IDS:
        fail(f"layer ids mismatch: {layer_ids}")
    if matrix.get("layer_ids") != REQUIRED_LAYER_IDS:
        fail("top-level layer_ids mismatch")

    cx_by_id = {cx["id"]: cx for cx in matrix["counterexamples"]}
    for layer in layers:
        require_keys(
            layer,
            {
                "id",
                "title_zh",
                "title_en",
                "version",
                "reuses",
                "sources",
                "can_prove",
                "cannot_prove",
                "independent_counterexample",
            },
            f"layer {layer.get('id')}",
        )
        for side in ("local_cli", "shared_mcp", "team_pg"):
            if side not in layer["sources"]:
                fail(f"layer {layer['id']} missing source side {side}")
            require_keys(
                layer["sources"][side],
                {"classification", "entrypoints", "notes"},
                f"layer {layer['id']} source {side}",
            )
        if not layer["can_prove"] or not layer["cannot_prove"]:
            fail(f"layer {layer['id']} needs non-empty can/cannot prove lists")
        expected_cx = LAYER_CX[layer["id"]]
        if layer["independent_counterexample"] != expected_cx:
            fail(
                f"layer {layer['id']} counterexample id "
                f"{layer['independent_counterexample']} != {expected_cx}"
            )
        cx = cx_by_id.get(expected_cx)
        if not cx:
            fail(f"missing counterexample {expected_cx}")
        if cx.get("layer") != layer["id"]:
            fail(f"{expected_cx} layer field {cx.get('layer')} != {layer['id']}")
        fixture = MIRROR / "counterexamples" / f"{expected_cx.lower().replace('_', '-')}.json"
        if not fixture.is_file():
            fail(f"missing fixture {fixture}")
        if load(fixture)["id"] != expected_cx:
            fail(f"fixture id mismatch in {fixture}")

    hosts = matrix["host_modes"]
    if len(hosts) != 2:
        fail(f"expected 2 host modes, got {len(hosts)}")
    host_ids = [host["id"] for host in hosts]
    if host_ids != REQUIRED_HOST_IDS:
        fail(f"host mode ids mismatch: {host_ids}")

    owners = set()
    for host in hosts:
        require_keys(
            host,
            {
                "id",
                "title_zh",
                "title_en",
                "version",
                "unique_write_owner",
                "host_owns",
                "awr_owns_writes",
                "may_create",
                "must_not",
                "independent_counterexample",
            },
            f"host {host.get('id')}",
        )
        owners.add(host["unique_write_owner"])
        expected_cx = HOST_CX[host["id"]]
        if host["independent_counterexample"] != expected_cx:
            fail(f"host {host['id']} counterexample mismatch")
        cx = cx_by_id.get(expected_cx)
        if not cx or cx.get("host_mode") != host["id"]:
            fail(f"host counterexample {expected_cx} missing or mis-tagged")
        fixture = MIRROR / "counterexamples" / f"{expected_cx.lower().replace('_', '-')}.json"
        if not fixture.is_file():
            fail(f"missing fixture {fixture}")

    if len(owners) != 2:
        fail(f"host modes must have distinct unique_write_owner, got {owners}")

    component = next(h for h in hosts if h["id"] == "component_only")
    forbidden = set(component["must_not"])
    for needle in (
        "create_host_work",
        "create_host_run",
        "create_host_owner",
        "hold_second_work_state",
    ):
        if needle not in forbidden:
            fail(f"component_only must forbid {needle}")
    if component["may_create"]:
        fail("component_only may_create must be empty (no second work state)")

    separations = matrix["separations"]
    for key in (
        "context_complete_but_dependencies_incomplete",
        "readable_but_not_writable",
        "entry_semantics_not_mixed",
    ):
        if key not in separations:
            fail(f"missing separation {key}")
    if not separations["context_complete_but_dependencies_incomplete"].get("expressible"):
        fail("context_complete_but_dependencies_incomplete must be expressible")
    if not separations["readable_but_not_writable"].get("expressible"):
        fail("readable_but_not_writable must be expressible")

    for cx_id in EXTRA_CX:
        if cx_id not in cx_by_id:
            fail(f"missing counterexample {cx_id}")
        fixture = MIRROR / "counterexamples" / f"{cx_id.lower().replace('_', '-')}.json"
        if not fixture.is_file():
            fail(f"missing fixture {fixture}")

    premises = "\n".join(matrix["premises"]).lower()
    for needle in (
        "unknown must never default to success",
        "readable must never imply writable",
        "context_completeness must never imply",
        "silently degrade",
    ):
        if needle not in premises:
            fail(f"premises missing: {needle}")

    compat = matrix["compatibility"]
    if not compat.get("keep_old_outputs"):
        fail("compatibility.keep_old_outputs must be true")
    forbidden = set(compat.get("negotiation", {}).get("forbidden", []))
    for needle in (
        "silent_degrade_when_required_capability_missing",
        "default_unknown_layer_status_to_success",
    ):
        if needle not in forbidden:
            fail(f"compatibility must forbid {needle}")

    gates = matrix["gates"]
    for g in (
        "evo_011_started",
        "dec_040_started",
        "dec_041_started",
        "mac_work_complete",
        "paid_or_native_agent_runs_required",
    ):
        if gates.get(g) is not False:
            fail(f"gate {g} must be false")

    for label, path in (
        ("host-contract.md", DOCS[0]),
        ("cli-mcp-contract.md", DOCS[1]),
        ("evolution-semantic-contract.md", DOCS[2]),
    ):
        text = path.read_text()
        for needle in (
            "work_readiness",
            "execution_admission",
            "context_completeness",
            "delivery_observation",
            "completion_validity",
            "runtime_delegated",
            "component_only",
            "AWR-EVO-010",
        ):
            if needle not in text:
                fail(f"{label} missing freeze marker {needle}")

    classes = set()
    for row in matrix["field_source_audit"]:
        classes.add(row["local_cli"])
        classes.add(row["shared_mcp"])
        classes.add(row["team_pg"])
    if not any("shared" in c for c in classes):
        fail("field_source_audit missing shared classifications")
    if not any("transformed" in c for c in classes):
        fail("field_source_audit missing transformed classifications")
    if not any("unsupported" in c for c in classes):
        fail("field_source_audit missing unsupported classifications")

    am = matrix["acceptance_mapping"]
    for key in ("1", "2", "3", "4"):
        if key not in am or not am[key].get("proof_refs"):
            fail(f"acceptance_mapping.{key} incomplete")

    # hash integrity: recompute excluding matrix_hash
    body = {k: v for k, v in matrix.items() if k != "matrix_hash"}
    canon = json.dumps(body, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    import hashlib
    expected = "sha256:" + hashlib.sha256(canon.encode()).hexdigest()
    if matrix["matrix_hash"] != expected:
        fail(f"matrix_hash mismatch: recorded={matrix['matrix_hash']} expected={expected}")

    print("OK: AWR-EVO-010 semantic contract matrix")
    print(f"  layers={','.join(layer_ids)}")
    print(f"  host_modes={','.join(host_ids)}")
    print(f"  counterexamples={len(cx_by_id)}")
    print(f"  matrix_hash={matrix['matrix_hash']}")
    print("  evo_011/dec_040/dec_041/mac_complete=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
