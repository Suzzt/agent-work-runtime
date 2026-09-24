#!/usr/bin/env python3
import importlib.util
import json
import shutil
import sys
import unittest
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SUPPORT = ROOT / "tests/fixtures/workstreams/parallel-handoff-biz/support.py"


def load_support():
    spec = importlib.util.spec_from_file_location("ws051_biz_support", SUPPORT)
    mod = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(mod)
    return mod


class FixtureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.support = load_support()
        cls.catalog, cls.contract, cls.specs = cls.support.load_bundle()

    def test_six_scenarios(self):
        self.assertEqual(len(self.specs), 6)
        self.assertEqual(
            set(self.specs),
            {"WS-BIZ-01", "WS-BIZ-02", "WS-BIZ-03", "WS-BIZ-04", "WS-BIZ-05", "WS-BIZ-06"},
        )

    def test_hard_gates_unique(self):
        self.assertEqual(set(self.catalog["hard_gates"]), set(self.support.HARD_GATES))
        seen = {row["hard_gate"] for row in self.catalog["scenarios"]}
        self.assertEqual(seen, set(self.support.HARD_GATES))

    def test_named_clients_and_web(self):
        self.assertEqual(set(self.contract["named_clients"]), {"codex_cli", "claude_code"})
        self.assertEqual(set(self.contract["required_surfaces"]), {"codex_cli", "claude_code", "team_web"})
        for sid, (scenario, _, result) in self.specs.items():
            self.assertEqual(set(scenario["named_clients"]), {"codex_cli", "claude_code"}, sid)
            self.assertIn("team_web", result["surfaces"], sid)

    def test_independent_reviewer_binding(self):
        for sid, (scenario, _, _) in self.specs.items():
            b = scenario["person_bindings"]
            self.assertNotEqual(b["executor"], b["reviewer"], sid)
            if scenario.get("same_person_dual_agent"):
                self.assertEqual(b["executor"], b["peer"], sid)
            else:
                self.assertNotEqual(b["executor"], b["peer"], sid)
                self.assertNotEqual(b["peer"], b["reviewer"], sid)

    def test_prompt_leak_guard(self):
        for sid, (scenario, _, _) in self.specs.items():
            for text in [scenario["initial_request"], *(f["request"] for f in scenario["followups"])]:
                self.assertEqual(self.support.client_input_violations(text), [], sid)

    def test_gates_inventory(self):
        self.assertEqual(len(self.support.GATES), 8)
        for sid, (scenario, _, _) in self.specs.items():
            self.assertEqual(scenario["required_gates"], self.support.GATES, sid)
            self.assertEqual(scenario["scope_revision"], 2, sid)

    def test_verify_main_blocks_live_gates(self):
        biz = ROOT / "tests/fixtures/workstreams/parallel-handoff-biz"
        sys.path.insert(0, str(biz))
        try:
            spec = importlib.util.spec_from_file_location("ws051_verify", biz / "verify.py")
            verify = importlib.util.module_from_spec(spec)
            assert spec.loader is not None
            spec.loader.exec_module(verify)
            evidence = ROOT / ".local/pr145-verify-test/scope-r2"
            shutil.rmtree(evidence.parent, ignore_errors=True)
            try:
                buf = StringIO()
                with redirect_stdout(buf):
                    rc = verify.main(["--evidence-root", str(evidence)])
                self.assertEqual(rc, 0)
                payload = json.loads(buf.getvalue())
                self.assertTrue(payload["passed_fixture_verification"])
                self.assertFalse(payload["live_pass"])
                self.assertEqual(payload["status"], "blocked_pending_trial_participants")
                self.assertEqual(len(payload["scenarios"]), 6)
            finally:
                shutil.rmtree(evidence.parent, ignore_errors=True)
        finally:
            sys.path.remove(str(biz))

    def test_ws_biz_04_same_person_dual_agent(self):
        scenario, directory, _ = self.specs["WS-BIZ-04"]
        self.assertTrue(scenario["same_person_dual_agent"])
        graph = self.support.read_json(directory / "work-graph.json")
        agent_nodes = [n for n in graph["nodes"] if n["role"] in {"executor", "executor_peer"}]
        persons = {n["person_id"] for n in agent_nodes}
        adapters = {n["client_adapter"] for n in agent_nodes}
        self.assertEqual(len(persons), 1)
        self.assertTrue({"codex_cli", "claude_code"}.issubset(adapters))


if __name__ == "__main__":
    unittest.main()
