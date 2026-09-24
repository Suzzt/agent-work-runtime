#!/usr/bin/env python3
"""Unit tests for DEC-060 mvp-acceptance pack invariants (offline)."""
from __future__ import annotations

import json
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
PACK = ROOT / "tests" / "fixtures" / "assessment" / "mvp-acceptance"

FIRST_BATCH = [
    "AWR-DEC-010",
    "AWR-DEC-011",
    "AWR-DEC-012",
    "AWR-DEC-013",
    "AWR-DEC-020",
    "AWR-DEC-021",
    "AWR-DEC-022",
    "AWR-DEC-060",
]


class MvpAcceptanceTests(unittest.TestCase):
    def test_eight_items_independently_counted(self):
        manifest = json.loads((PACK / "manifest.json").read_text())
        self.assertEqual(manifest["first_batch_count"], 8)
        works = [i["work"] for i in manifest["items"]]
        self.assertEqual(works, FIRST_BATCH)
        self.assertEqual([i["index"] for i in manifest["items"]], list(range(1, 9)))

    def test_non_claims_cover_required_ids(self):
        non = json.loads((PACK / "non-claims.json").read_text())
        ids = {d["id"] for d in non["does_not_prove"]}
        self.assertEqual(
            ids,
            {"full_14_item_suite", "AUTO", "Team", "native_host", "released_version"},
        )

    def test_perf_raw_retained_without_roi(self):
        raw = json.loads((PACK / "perf-raw" / "offline-parse-hash-samples.json").read_text())
        self.assertGreaterEqual(len(raw["samples"]), 30)
        self.assertFalse(raw["environment"]["model_configured"])
        self.assertIn("derive_dollar_savings", raw["forbid"])
        budget = json.loads((PACK / "budget-crosscheck.json").read_text())
        self.assertFalse(budget["roi_claims_invented"])

    def test_follow_ons_do_not_start_dec040_or_evo(self):
        follow = json.loads((PACK / "follow-ons.json").read_text())
        self.assertFalse(follow["dec_040_started"])
        self.assertFalse(follow["dec_041_started"])
        self.assertFalse(follow["evo_000_started"])

    def test_gate_checklist_prior_all_pass(self):
        checklist = json.loads((PACK / "gate-checklist.json").read_text())
        self.assertTrue(checklist["prior_all_pass"])
        self.assertEqual(len(checklist["prior_items_rechecked"]), 7)


if __name__ == "__main__":
    unittest.main()
