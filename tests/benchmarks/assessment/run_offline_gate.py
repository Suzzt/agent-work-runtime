#!/usr/bin/env python3
"""Thin harness to re-run the DEC-060 offline explain gate (read-only, no model/network).

Default: artifact + Python verifies (fast).
Pass --full to also invoke Cargo assessment offline/explain tests when cargo is available.
"""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def run(cmd: list[str], cwd: Path | None = None) -> None:
    print("+", " ".join(cmd), flush=True)
    r = subprocess.run(cmd, cwd=cwd or ROOT)
    if r.returncode != 0:
        raise SystemExit(r.returncode)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--full",
        action="store_true",
        help="Also run cargo assessment offline/explain tests (requires toolchain).",
    )
    args = ap.parse_args()

    py = [
        HERE / "prove_acceptance_dec060.py",
    ]
    for script in py:
        run([sys.executable, str(script)])

    if args.full:
        if not shutil.which("cargo"):
            print("cargo not found; --full skipped cargo portion", file=sys.stderr)
            return 1
        cargo_cmds = [
            ["cargo", "test", "-p", "awr-runtime", "--lib", "assessment_", "--offline"],
            ["cargo", "test", "-p", "awr-runtime", "--test", "assessment_offline", "--offline"],
            ["cargo", "test", "-p", "awr-cli", "--test", "assessment_offline_cli", "--offline"],
            ["cargo", "test", "-p", "awr-cli", "--test", "assessment_explain_cli", "--offline"],
            ["cargo", "test", "-p", "awr-mcp", "--test", "assessment_explain_mcp", "--offline"],
            ["cargo", "test", "-p", "awr-runtime", "--test", "explanation_chain", "--offline"],
        ]
        for cmd in cargo_cmds:
            run(cmd)

    pack = ROOT / "tests" / "fixtures" / "assessment" / "mvp-acceptance" / "manifest.json"
    m = json.loads(pack.read_text())
    print(
        json.dumps(
            {
                "gate": "AWR-DEC-060",
                "first_batch_count": m["first_batch_count"],
                "full": args.full,
                "evo_000_started": m["evo_000_started"],
                "dec_040_started": m["dec_040_started"],
                "status": "ok",
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
