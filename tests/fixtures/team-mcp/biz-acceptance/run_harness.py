#!/usr/bin/env python3
"""Trial-stack harness entry for Team MCP BIZ acceptance (AWR-TMCP-051).

Capabilities in this agent environment:
- Structural fixture verification (always)
- Optional read-only PG connectivity probe when AWR_TEAM_TEST_DATABASE_URL is set
- Does NOT mint member credentials, start named clients, or forge independent review

Live multi-person gates remain 待验 unless a human team supplies evidence paths.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from datetime import datetime
from pathlib import Path
from urllib.parse import urlparse
from zoneinfo import ZoneInfo

from support import BASE, GATES, ROOT, load_bundle, read_json
from verify import main as verify_main


def probe_pg(url: str) -> dict:
    parsed = urlparse(url)
    host = parsed.hostname or ""
    if host not in {"127.0.0.1", "localhost", "::1"}:
        return {"ok": False, "reason": "refusing non-loopback PG probe for acceptance harness"}
    try:
        out = subprocess.check_output(
            ["psql", url, "-v", "ON_ERROR_STOP=1", "-c", "select 1 as ok;"],
            text=True,
            stderr=subprocess.STDOUT,
            timeout=15,
        )
        return {"ok": "ok" in out, "detail": "loopback select 1 succeeded", "read_only": True}
    except Exception as exc:  # noqa: BLE001
        return {"ok": False, "reason": str(exc)}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-root", type=Path, default=ROOT / ".local/awr-team-mcp-acceptance-v1")
    parser.add_argument("--skip-pg-probe", action="store_true")
    args = parser.parse_args(argv)

    # Always refresh structural reports first.
    rc = verify_main(["--evidence-root", str(args.evidence_root), "--write-reports"])
    if rc != 0:
        return rc

    catalog, contract, specs = load_bundle()
    now = datetime.now(ZoneInfo("Asia/Taipei")).isoformat(timespec="seconds")
    pg = {"ok": False, "reason": "not probed"}
    if not args.skip_pg_probe:
        url = os.environ.get("AWR_TEAM_TEST_DATABASE_URL")
        if url:
            pg = probe_pg(url)
        else:
            pg = {"ok": False, "reason": "AWR_TEAM_TEST_DATABASE_URL unset"}

    harness = {
        "work": "AWR-TMCP-051",
        "schema": "team_mcp_acceptance",
        "recorded_at_cst": now,
        "contract_id": contract["contract_id"],
        "fixture_verification": "passed",
        "pg_probe": pg,
        "named_clients": contract["named_clients"],
        "trial_stack": {
            "deploy_pack": "docs/reference/team-deploy-pack.md",
            "member_handoff": "docs/reference/team-member-handoff.md",
            "client_guides": [
                "docs/integrations/team-mcp-codex-cli.md",
                "docs/integrations/team-mcp-claude-code.md",
            ],
            "spun_up_in_this_run": False,
            "reason": "No member credentials / two human operators / independent reviewer available on this box",
        },
        "scenarios": {},
        "status": "blocked_pending_trial_participants",
        "missing_resources": [
            "two distinct developer person credentials bound to codex_cli and claude_code",
            "independent reviewer person credential (not the executor)",
            "HTTPS Team MCP trial endpoint with verified boundary from TMCP-041 deploy pack",
            "human-operated natural-language initiations and two business followups per scenario",
            "real PR delivery commits with remote SHAs per scenario",
        ],
        "gates_policy": "待验 is not pass; do not SQL-mutate business state for acceptance",
    }

    for sid, (scenario, directory, result) in specs.items():
        report = read_json(args.evidence_root / directory.name / "report.json")
        gate_matrix = {
            gate: {
                "status": report["gates"][gate]["status"],
                "evidence_paths": report["gates"][gate].get("evidence_paths", []),
                "blocker": report["gates"][gate].get("blocker"),
            }
            for gate in GATES
        }
        harness["scenarios"][sid] = {
            "namespace": scenario["namespace"],
            "persons": result["persons"],
            "clients": result["clients"],
            "gates": gate_matrix,
            "passed_gates": [g for g, row in gate_matrix.items() if row["status"] == "passed"],
            "pending_gates": [g for g, row in gate_matrix.items() if row["status"] == "待验"],
        }

    out = args.evidence_root / "harness-run.json"
    out.write_text(json.dumps(harness, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "status": harness["status"],
        "fixture_verification": "passed",
        "pg_probe_ok": pg.get("ok"),
        "scenarios": list(specs),
        "report": str(out.relative_to(ROOT)),
    }, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
