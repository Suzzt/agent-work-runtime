#!/usr/bin/env python3
"""Structural verification for Team MCP BIZ acceptance fixtures (AWR-TMCP-051).

This does NOT mark live multi-person gates as passed. It only proves the fixture
pack + report templates are coherent and leak-free.
"""
from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

from support import BASE, GATES, ROOT, digest, load_bundle, read_json, require


def rel_or_abs(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)



def build_report_template_payload(
    *,
    template: dict,
    now: str,
    directory: Path,
    result: dict,
) -> dict:
    report = dict(template)
    report["recorded_at_cst"] = now
    report["fixture_verification"] = {
        "passed": True,
        "work_keys": result["work_keys"],
        "persons": result["persons"],
        "clients": result["clients"],
        "scenario_sha256": digest(directory / "scenario.json"),
    }
    report["status"] = "blocked_pending_trial_participants"
    report["blockers"] = [
        "No live trial stack with member credentials for two developer persons + independent reviewer",
        "Named clients codex_cli and claude_code not operated by distinct humans in this run",
        "Cannot honestly pass independent_review without a distinct reviewer person session",
    ]
    return report


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--evidence-root",
        type=Path,
        default=ROOT / ".local/awr-team-mcp-acceptance-v1",
        help="Directory for per-scenario report.json files",
    )
    parser.add_argument(
        "--write-reports",
        action="store_true",
        help=(
            "Initialize missing report.json files under evidence-root from templates. "
            "Existing reports (including filled evidence_paths) are preserved."
        ),
    )
    parser.add_argument(
        "--reset-reports",
        action="store_true",
        help=(
            "Explicitly replace every report.json under evidence-root with a fresh "
            "pending template. Destructive; never used by the normal harness path."
        ),
    )
    args = parser.parse_args(argv)
    if args.reset_reports and not args.write_reports:
        # reset implies write capability
        args.write_reports = True

    catalog, contract, specs = load_bundle()
    gate_contract = read_json(BASE / "gate-contract.json")
    require(set(gate_contract["gates"]) == set(GATES), "gate-contract mismatch")

    now = datetime.now(ZoneInfo("Asia/Taipei")).isoformat(timespec="seconds")
    summary = {
        "work": "AWR-TMCP-051",
        "contract_id": contract["contract_id"],
        "checked_at_cst": now,
        "fixture_pass": True,
        "live_pass": False,
        "scenarios": {},
        "named_clients_required": contract["named_clients"],
        "required_gates": GATES,
        "honest_status": "blocked_pending_trial_participants",
    }

    for sid, (scenario, directory, result) in specs.items():
        report_path = args.evidence_root / directory.name / "report.json"
        template = read_json(directory / "report.template.json")
        require(template["scenario_id"] == sid, "report template scenario mismatch")
        require(set(template["gates"]) == set(GATES), "report template gates")
        for gate, row in template["gates"].items():
            require(row["status"] == "待验", f"{sid}/{gate} must remain 待验 in template")
            require(row.get("evidence_paths") == [], f"{sid}/{gate} must not claim evidence")

        if args.write_reports:
            args.evidence_root.mkdir(parents=True, exist_ok=True)
            out_dir = args.evidence_root / directory.name
            out_dir.mkdir(parents=True, exist_ok=True)
            report_path = out_dir / "report.json"
            should_write = args.reset_reports or not report_path.exists()
            if should_write:
                report = build_report_template_payload(
                    template=template, now=now, directory=directory, result=result
                )
                report_path.write_text(
                    json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
                )

        summary["scenarios"][sid] = {
            "namespace": scenario["namespace"],
            "fixture": rel_or_abs(directory),
            "fixture_ok": True,
            "live_gates_passed": 0,
            "live_gates_pending": len(GATES),
            "report": rel_or_abs(report_path) if report_path.exists() else None,
            "persons": result["persons"],
            "clients": result["clients"],
        }

    out = args.evidence_root / "verify-summary.json"
    args.evidence_root.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "passed_fixture_verification": True,
        "live_pass": False,
        "scenarios": list(specs),
        "summary": rel_or_abs(out),
        "status": "blocked_pending_trial_participants",
    }, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
