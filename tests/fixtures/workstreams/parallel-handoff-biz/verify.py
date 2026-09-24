#!/usr/bin/env python3
"""Structural verification for workstream parallel-handoff BIZ fixtures (AWR-WS-051).

Does NOT mark live multi-person / multi-client / Web gates as passed.
"""
from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

from support import BASE, GATES, HARD_GATES, ROOT, digest, load_bundle, materialize_deliverable, read_json, require


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--evidence-root",
        type=Path,
        default=ROOT / ".local/awr-workstream-implementation-20260921/business/scope-r2",
    )
    parser.add_argument("--write-reports", action="store_true")
    parser.add_argument("--materialize", action="store_true", help="Copy fixtures into deliverable layout")
    args = parser.parse_args(argv)

    catalog, contract, specs = load_bundle()
    gate_contract = read_json(BASE / "gate-contract.json")
    require(set(gate_contract["gates"]) == set(GATES), "gate-contract mismatch")

    if args.materialize:
        materialize_deliverable(args.evidence_root)

    now = datetime.now(ZoneInfo("Asia/Taipei")).isoformat(timespec="seconds")
    summary = {
        "work": "AWR-WS-051",
        "scope_revision": 2,
        "contract_id": contract["contract_id"],
        "checked_at_cst": now,
        "fixture_pass": True,
        "live_pass": False,
        "hard_gates": HARD_GATES,
        "scenarios": {},
        "named_clients_required": contract["named_clients"],
        "required_surfaces": contract["required_surfaces"],
        "required_gates": GATES,
        "honest_status": "blocked_pending_trial_participants",
    }

    for sid, (scenario, directory, result) in specs.items():
        slug = directory.name
        report_path = args.evidence_root / slug / "report.json"
        template = read_json(directory / "report.template.json")
        require(template["scenario_id"] == sid, "report template scenario mismatch")
        require(set(template["gates"]) == set(GATES), "report template gates")
        for gate, row in template["gates"].items():
            require(row["status"] == "待验", f"{sid}/{gate} must remain 待验 in template")
            require(row.get("evidence_paths") == [], f"{sid}/{gate} must not claim evidence")

        if args.write_reports:
            args.evidence_root.mkdir(parents=True, exist_ok=True)
            out_dir = args.evidence_root / slug
            out_dir.mkdir(parents=True, exist_ok=True)
            existing = None
            if report_path.exists():
                existing = read_json(report_path)
                # Preserve any live-filled evidence if present
                filled = any(
                    (existing.get("gates") or {}).get(g, {}).get("status") == "passed"
                    for g in GATES
                )
            else:
                filled = False
            if filled and existing is not None:
                report = existing
                report["fixture_verification"] = {
                    "passed": True,
                    "work_keys": result["work_keys"],
                    "persons": result["persons"],
                    "clients": result["clients"],
                    "surfaces": result["surfaces"],
                    "hard_gate": result["hard_gate"],
                    "scenario_sha256": digest(directory / "scenario.json"),
                }
                report["recorded_at_cst"] = now
            else:
                report = dict(template)
                report["recorded_at_cst"] = now
                report["fixture_verification"] = {
                    "passed": True,
                    "work_keys": result["work_keys"],
                    "persons": result["persons"],
                    "clients": result["clients"],
                    "surfaces": result["surfaces"],
                    "hard_gate": result["hard_gate"],
                    "scenario_sha256": digest(directory / "scenario.json"),
                }
                report["status"] = "blocked_pending_trial_participants"
                report["blockers"] = [
                    "No live trial with two named Agent clients (codex_cli + claude_code)",
                    "No human Team Web operator available on this sandbox box",
                    "Cannot honestly pass independent_review without a distinct reviewer person session",
                    "Per-scenario independent commit/remote SHA not produced by live delivery",
                ]
            report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

        summary["scenarios"][sid] = {
            "namespace": scenario["namespace"],
            "hard_gate": result["hard_gate"],
            "fixture": str(directory.relative_to(ROOT)),
            "fixture_ok": True,
            "live_gates_passed": 0,
            "live_gates_pending": len(GATES),
            "report": str(report_path.relative_to(ROOT)) if report_path.exists() else None,
            "persons": result["persons"],
            "clients": result["clients"],
            "surfaces": result["surfaces"],
        }

    out = args.evidence_root.parent / "verify-summary.json"
    args.evidence_root.mkdir(parents=True, exist_ok=True)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "passed_fixture_verification": True,
        "live_pass": False,
        "scenarios": list(specs),
        "hard_gates": HARD_GATES,
        "summary": str(out.relative_to(ROOT)),
        "status": "blocked_pending_trial_participants",
    }, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
