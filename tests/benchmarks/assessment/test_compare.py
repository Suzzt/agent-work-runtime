#!/usr/bin/env python3
"""Unit tests for DEC-013 compare statistics and hard-gate offset rejection."""
from __future__ import annotations

import json
import unittest
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import compare  # noqa: E402


class CompareTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.contract = json.loads((HERE / "contract.json").read_text())
        cls.budgets = json.loads((HERE / "budgets.json").read_text())
        cls.samples = json.loads((HERE / "sample-receipts.json").read_text())

    def test_p95_requires_thirty_samples(self):
        with self.assertRaises(ValueError):
            compare.nearest_rank_p95([1.0] * 29)

    def test_hard_fail_not_offset_by_average(self):
        report = compare.compare_pair(
            self.contract,
            self.budgets,
            self.samples["baseline_receipt"],
            self.samples["candidate_receipt_hard_fail_offset_attempt"],
        )
        self.assertFalse(report["passed"])
        self.assertFalse(report["families"]["structural_correctness"]["hard_pass"])
        self.assertTrue(report["families"]["structural_correctness"]["offset_attempt_rejected"])

    def _bound_budgets(self):
        budgets = json.loads(json.dumps(self.budgets))
        budgets["status"] = "frozen_with_baseline"
        budgets["dimensions"]["collect"]["measured_baseline"] = 10.0
        budgets["dimensions"]["judge"]["measured_baseline"] = 1.0
        budgets["dimensions"]["output"]["measured_baseline"] = 2.0
        return budgets

    def test_unbound_budget_does_not_pass(self):
        report = compare.compare_pair(
            self.contract,
            self.budgets,
            self.samples["baseline_receipt"],
            self.samples["candidate_receipt_ok"],
        )
        overhead = report["families"]["runtime_overhead"]
        self.assertFalse(report["passed"])
        self.assertFalse(overhead["baseline_bound"])
        self.assertFalse(overhead["evaluable"])
        self.assertEqual(overhead["ms_reasons"], ["baseline_not_bound"])
        self.assertFalse(overhead["zero_filled"])

    def test_missing_cost_is_not_zero_filled(self):
        candidate = json.loads(json.dumps(self.samples["candidate_receipt_ok"]))
        del candidate["costs"]["collect_ms"]
        report = compare.compare_pair(
            self.contract,
            self._bound_budgets(),
            self.samples["baseline_receipt"],
            candidate,
        )
        overhead = report["families"]["runtime_overhead"]
        self.assertFalse(report["passed"])
        self.assertIn("collect_ms", overhead["missing_fields"])
        self.assertNotIn("collect_ms_delta", overhead)
        self.assertFalse(overhead["zero_filled"])

    def test_ok_pair_passes_only_after_baseline_binding(self):
        report = compare.compare_pair(
            self.contract,
            self._bound_budgets(),
            self.samples["baseline_receipt"],
            self.samples["candidate_receipt_ok"],
        )
        self.assertTrue(report["passed"])
        self.assertTrue(report["families"]["runtime_overhead"]["evaluable"])
        self.assertEqual(
            set(report["families"]),
            {"structural_correctness", "runtime_overhead", "advisory_effectiveness"},
        )
        self.assertNotIn("model_success_rate", report)
        self.assertNotIn("dollar_savings", report)

    def test_bound_pair_over_ms_ceiling_fails(self):
        candidate = json.loads(json.dumps(self.samples["candidate_receipt_ok"]))
        candidate["costs"]["collect_ms"] = 1000.0
        report = compare.compare_pair(
            self.contract,
            self._bound_budgets(),
            self.samples["baseline_receipt"],
            candidate,
        )
        overhead = report["families"]["runtime_overhead"]
        self.assertFalse(report["passed"])
        self.assertTrue(overhead["evaluable"])
        self.assertFalse(overhead["ms_within_ceiling"])
        self.assertIn("collect", overhead["ms_reasons"])


if __name__ == "__main__":
    unittest.main()
