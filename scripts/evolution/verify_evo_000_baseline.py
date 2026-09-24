#!/usr/bin/env python3
"""Verify AWR-EVO-000 execution-baseline gate artifacts."""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LOCAL = ROOT / ".local/awr-evolution-20260919"
MIRROR = ROOT / "tests/fixtures/evolution/AWR-EVO-000"
MATRIX = ROOT / "docs/reference/team-v1-evidence-matrix.json"
HISTORICAL = ROOT / "docs/reference/team-v1-historical-agent-evidence-v1.json"
REQUIRED_BASELINE_KEYS = {
    "schema",
    "work",
    "construction_facts",
    "program_identity",
    "project_identity",
    "source_fingerprints",
    "access_modes",
    "preserved_denominators",
    "active_items",
    "unknown_executions",
    "path_occupancy",
    "dec_040_started",
    "dec_041_started",
}


def fail(msg: str) -> None:
    print(f"FAIL: {msg}")
    raise SystemExit(1)


def load(path: Path) -> dict:
    if not path.is_file():
        fail(f"missing {path}")
    return json.loads(path.read_text())


def main() -> int:
    local_json = LOCAL / "execution-baseline.json"
    mirror_json = MIRROR / "execution-baseline.json"
    local_md = LOCAL / "overlap-and-authority.md"
    mirror_md = MIRROR / "overlap-and-authority.md"

    for p in (mirror_json, mirror_md, MIRROR / "README.md"):
        if not p.is_file():
            fail(f"missing required artifact {p}")

    mirror = load(mirror_json)
    # The checked-in mirror is the gate. A private .local copy is optional, but
    # if it exists it must match the mirror. CI and clean checkouts have no .local.
    if local_json.is_file() or local_md.is_file():
        if not local_json.is_file() or not local_md.is_file():
            fail("local evolution baseline is partial; json and markdown must both exist")
        baseline = load(local_json)
        if baseline != mirror:
            fail("local baseline JSON differs from checked-in mirror")
        if local_md.read_text() != mirror_md.read_text():
            fail("local overlap markdown differs from checked-in mirror")
    else:
        baseline = mirror

    missing = REQUIRED_BASELINE_KEYS - set(baseline)
    if missing:
        fail(f"baseline missing keys: {sorted(missing)}")

    if baseline.get("work") != "AWR-EVO-000":
        fail("work key must be AWR-EVO-000")
    if baseline.get("schema") != "awr-evolution-execution-baseline/v1":
        fail("unexpected schema")

    cf = baseline["construction_facts"]
    for k in ("head_sha", "tree_sha", "branch", "working_tree"):
        if not cf.get(k):
            fail(f"construction_facts.{k} required")
    if len(cf["head_sha"]) != 40 or len(cf["tree_sha"]) != 40:
        fail("HEAD/tree must be full 40-char SHAs")

    prog = baseline["program_identity"]
    if not prog.get("workspace_package_version"):
        fail("program version required")
    if not prog.get("summary"):
        fail("program summary required")
    if not prog.get("source_file_fingerprints"):
        fail("program source fingerprints required")

    modes = baseline["access_modes"]
    for mode in ("personal_local", "team_pg"):
        if mode not in modes:
            fail(f"access mode {mode} missing")
        if not modes[mode].get("fact_owner"):
            fail(f"{mode}.fact_owner required")

    proj = baseline["project_identity"]
    if proj.get("authority_ledger") != "ledger/work-ledger.yaml":
        fail("special must keep ledger/work-ledger.yaml authority")
    if not proj.get("project_id"):
        fail("project_id required")

    den = baseline["preserved_denominators"]
    counts = den.get("team_v1_evidence_matrix_counts") or {}
    matrix = load(MATRIX)
    matrix_counts = matrix.get("counts") or {}
    if len(matrix.get("cases") or []) != matrix_counts.get("required"):
        fail("evidence matrix case count does not match counts.required")
    for key, expected in (
        ("required", 69),
        ("real_agent_accepted", 42),
        ("automated_partial", 69),
        ("automated_evidence_pending", 27),
    ):
        if counts.get(key) != expected:
            fail(f"Team V1 denominator {key} changed or missing: {counts.get(key)}")
        if matrix_counts.get(key) != expected:
            fail(f"evidence matrix {key} is {matrix_counts.get(key)}, expected {expected}")
    hist = den.get("team_v1_historical_agent_evidence") or {}
    recorded = str(hist.get("fingerprint") or "")
    digest = recorded.removeprefix("sha256:")
    actual = hashlib.sha256(HISTORICAL.read_bytes()).hexdigest()
    if not digest or digest != actual:
        fail("historical evidence fingerprint does not match the checked-in file")
    matrix_hist = (matrix.get("historical_agent_evidence") or {}).get("index_sha256")
    if matrix_hist != actual:
        fail("evidence matrix historical index hash does not match the file")
    if den.get("explicit_non_rewrite") is not True:
        fail("explicit_non_rewrite must be true")

    ue = baseline["unknown_executions"]
    if ue.get("mac_awr_execution_registry") != "unknown":
        fail("Mac execution registry must be unknown unless actually queried")

    related = baseline["active_items"].get("related_named_tasks") or {}
    for key in ("AWR-UX-001", "TEAM-P17", "QA-003"):
        item = related.get(key) or {}
        if item.get("action") != "do_not_modify_owner_or_status":
            fail(f"{key} must remain do_not_modify_owner_or_status")
        if item.get("mac_live_status") != "unknown":
            fail(f"{key} mac_live_status must be unknown without Mac query")

    if baseline.get("dec_040_started") is not False:
        fail("dec_040_started must be false")
    if baseline.get("dec_041_started") is not False:
        fail("dec_041_started must be false")

    md = mirror_md.read_text()
    for needle in (
        "Unique work authority",
        "Requirement对照",
        "Path occupancy",
        "do not modify",
        "dec_040_started=false",
        "required`: 69",
    ):
        if needle not in md:
            fail(f"overlap doc missing section/marker: {needle}")
    occ = baseline["path_occupancy"]
    for path, info in occ.items():
        if info.get("conflict") is True:
            fail(f"path occupancy conflict recorded for {path}; refuse green gate")

    print("OK: AWR-EVO-000 baseline + overlap authority gate")
    print(f"  head={cf['head_sha']}")
    print(f"  tree={cf['tree_sha']}")
    print(f"  modes={','.join(k for k in modes if k.endswith(('local','pg')) or k in ('personal_local','team_pg'))}")
    print(f"  team_v1_required={counts['required']} real_agent_accepted={counts['real_agent_accepted']}")
    print("  dec_040_started=false dec_041_started=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
