#!/usr/bin/env python3
"""Local regressions for TMCP-051 harness CR P2s (preserve reports; redact probe URL)."""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import run_harness  # noqa: E402
import verify  # noqa: E402
from support import GATES, load_bundle  # noqa: E402


class PreserveReportsTests(unittest.TestCase):
    def test_write_reports_preserves_filled_evidence(self) -> None:
        _, _, specs = load_bundle()
        sid, (_scenario, directory, _result) = next(iter(specs.items()))
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            out_dir = root / directory.name
            out_dir.mkdir(parents=True)
            report_path = out_dir / "report.json"
            template = json.loads((directory / "report.template.json").read_text(encoding="utf-8"))
            filled = dict(template)
            first_gate = next(iter(GATES))
            filled["gates"][first_gate] = {
                "status": "passed",
                "evidence_paths": ["evidence/human-trial.md"],
                "blocker": None,
            }
            report_path.write_text(json.dumps(filled, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

            rc = verify.main(["--evidence-root", str(root), "--write-reports"])
            self.assertEqual(rc, 0)
            after = json.loads(report_path.read_text(encoding="utf-8"))
            self.assertEqual(after["gates"][first_gate]["status"], "passed")
            self.assertEqual(after["gates"][first_gate]["evidence_paths"], ["evidence/human-trial.md"])

            # Explicit reset is allowed and destructive.
            rc = verify.main(["--evidence-root", str(root), "--write-reports", "--reset-reports"])
            self.assertEqual(rc, 0)
            reset = json.loads(report_path.read_text(encoding="utf-8"))
            self.assertEqual(reset["gates"][first_gate]["status"], "待验")
            self.assertEqual(reset["gates"][first_gate].get("evidence_paths"), [])


class ProbeRedactionTests(unittest.TestCase):
    def test_failed_probe_omits_password(self) -> None:
        secret = "s3cret-password-should-not-leak"
        url = f"postgresql://trial_user:{secret}@127.0.0.1:5432/awr_team"

        def boom(*_a, **_k):
            raise subprocess.CalledProcessError(
                returncode=2,
                cmd=["psql", url, "-c", "select 1"],
                output=f"connection failed for {url}",
            )

        with mock.patch("run_harness.subprocess.check_output", side_effect=boom):
            result = run_harness.probe_pg(url)
        blob = json.dumps(result)
        self.assertNotIn(secret, blob)
        self.assertNotIn(url, blob)
        self.assertFalse(result["ok"])
        self.assertEqual(result["reason"], "psql_failed")

    def test_timeout_probe_omits_password(self) -> None:
        secret = "another-secret-value"
        url = f"postgresql://trial_user:{secret}@127.0.0.1:5432/awr_team"

        def boom(*_a, **_k):
            raise subprocess.TimeoutExpired(cmd=["psql", url], timeout=15)

        with mock.patch("run_harness.subprocess.check_output", side_effect=boom):
            result = run_harness.probe_pg(url)
        blob = json.dumps(result)
        self.assertNotIn(secret, blob)
        self.assertEqual(result["reason"], "psql_timeout")

    def test_redact_database_url(self) -> None:
        url = "postgresql://trial_user:s3cret@127.0.0.1:5432/awr_team"
        redacted = run_harness.redact_database_url(url)
        self.assertNotIn("s3cret", redacted)
        self.assertIn("trial_user:***@", redacted)


if __name__ == "__main__":
    unittest.main()
