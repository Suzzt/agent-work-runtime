#!/usr/bin/env python3
import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SUPPORT = ROOT / "tests/fixtures/team-mcp/biz-acceptance/support.py"


def load_support():
    spec = importlib.util.spec_from_file_location("tmcp_biz_support", SUPPORT)
    mod = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(mod)
    return mod


class FixtureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.support = load_support()
        cls.catalog, cls.contract, cls.specs = cls.support.load_bundle()

    def test_four_scenarios(self):
        self.assertEqual(len(self.specs), 4)
        self.assertEqual(set(self.specs), {"TMCP-BIZ-01", "TMCP-BIZ-02", "TMCP-BIZ-03", "TMCP-BIZ-04"})

    def test_named_clients(self):
        self.assertEqual(set(self.contract["named_clients"]), {"codex_cli", "claude_code"})
        for sid, (scenario, _, result) in self.specs.items():
            self.assertEqual(set(scenario["named_clients"]), {"codex_cli", "claude_code"}, sid)
            self.assertGreaterEqual(len(result["persons"]), 3, sid)

    def test_independent_reviewer_binding(self):
        for sid, (scenario, directory, _) in self.specs.items():
            b = scenario["person_bindings"]
            self.assertNotEqual(b["executor"], b["reviewer"], sid)
            self.assertNotEqual(b["peer"], b["reviewer"], sid)
            self.assertNotEqual(b["executor"], b["peer"], sid)

    def test_prompt_leak_guard(self):
        for sid, (scenario, _, _) in self.specs.items():
            for text in [scenario["initial_request"], *(f["request"] for f in scenario["followups"])]:
                self.assertEqual(self.support.client_input_violations(text), [], sid)

    def test_gates_inventory(self):
        self.assertEqual(len(self.support.GATES), 8)
        for sid, (scenario, _, _) in self.specs.items():
            self.assertEqual(scenario["required_gates"], self.support.GATES, sid)


if __name__ == "__main__":
    unittest.main()
