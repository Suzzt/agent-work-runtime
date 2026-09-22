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

    def test_ok_pair_passes_with_separate_families(self):
        report = compare.compare_pair(
            self.contract,
            self.budgets,
            self.samples["baseline_receipt"],
            self.samples["candidate_receipt_ok"],
        )
        self.assertTrue(report["passed"])
        self.assertEqual(
            set(report["families"]),
            {"structural_correctness", "runtime_overhead", "advisory_effectiveness"},
        )
        self.assertNotIn("model_success_rate", report)
        self.assertNotIn("dollar_savings", report)


if __name__ == "__main__":
    unittest.main()
