"""Lightweight artifact checks for AWR-TMCP-041."""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CHECK = ROOT / "scripts" / "team-deploy" / "check_deploy_pack.py"


class DeployPackArtifactsTest(unittest.TestCase):
    def test_check_deploy_pack_passes(self) -> None:
        proc = subprocess.run(
            [sys.executable, str(CHECK)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(
            proc.returncode,
            0,
            msg=f"stdout:\n{proc.stdout}\nstderr:\n{proc.stderr}",
        )
        self.assertIn("deploy pack OK", proc.stdout)

    def test_two_client_configs_exist(self) -> None:
        clients = ROOT / "examples" / "team-mcp-deploy" / "clients"
        self.assertTrue((clients / "codex_cli.mcp.toml.example").is_file())
        self.assertTrue((clients / "claude_code.mcp.json.example").is_file())

    def test_ops_scripts_exist(self) -> None:
        scripts = ROOT / "scripts" / "team-deploy"
        for name in ("migrate.sh", "first-admin.sh", "backup.sh", "version-check.sh"):
            self.assertTrue((scripts / name).is_file(), msg=name)


if __name__ == "__main__":
    unittest.main()
