#!/usr/bin/env python3
"""Prove AWR-DEC-060's three acceptance criteria from frozen artifacts (offline).

AC1: Cross-check DEC-010..022 acceptance evidence; first-batch 8 items counted independently.
AC2: Offline/no-model path, CLI/MCP parity, hard-reject, kill-switch fallback; perf raw retained.
AC3: Proves current offline explain only — explicit non-claims for full 14 / AUTO / Team /
     native host / released version.
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
PACK = ROOT / "tests" / "fixtures" / "assessment" / "mvp-acceptance"
CE = ROOT / "tests" / "fixtures" / "assessment" / "counterexamples"
REPLAY = ROOT / "tests" / "fixtures" / "assessment" / "replay"
REF = ROOT / "docs" / "reference" / "assessment.md"
BENCH_DOC = ROOT / "docs" / "benchmarks" / "assessment.md"

REQUIRED_NON_CLAIMS = {
    "full_14_item_suite",
    "AUTO",
    "Team",
    "native_host",
    "released_version",
}

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


def _run(script: Path) -> None:
    r = subprocess.run([sys.executable, str(script)], capture_output=True, text=True)
    assert r.returncode == 0, f"{script.name} failed\n{r.stderr}\n{r.stdout}"


def _require_ancestor(sha: str) -> None:
    r = subprocess.run(
        ["git", "merge-base", "--is-ancestor", sha, "HEAD"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert r.returncode == 0, f"{sha} is not in this tree"


def ac1_cross_check_eight_independent() -> bool:
    manifest = json.loads((PACK / "manifest.json").read_text())
    checklist = json.loads((PACK / "gate-checklist.json").read_text())
    assert manifest["schema_id"] == "awr-assessment-mvp-acceptance-v1"
    assert manifest["work"] == "AWR-DEC-060"
    assert manifest["first_batch_count"] == 8
    assert len(manifest["items"]) == 8
    works = [i["work"] for i in manifest["items"]]
    assert works == FIRST_BATCH
    # Independently counted: each item has its own index 1..8
    assert [i["index"] for i in manifest["items"]] == list(range(1, 9))
    assert checklist["first_batch_count"] == 8
    assert checklist["prior_all_pass"] is True
    assert len(checklist["prior_items_rechecked"]) == 7
    for row in checklist["prior_items_rechecked"]:
        assert row["independently_counted"] is True
        assert row["acceptance_pass"] is True
        assert row["artifacts_present"] is True
        assert row["acceptance_key_count"] >= 3
        receipt = json.loads((PACK / "prior-acceptance" / f"{row['work']}.json").read_text())
        assert receipt["work"] == row["work"]
        assert receipt["pr_url"]
        assert receipt["head_sha"] == row["head_sha"]
        _require_ancestor(receipt["head_sha"])
        assert receipt["independently_rechecked_by"] == "AWR-DEC-060"
    # Corpus marker
    ce = json.loads((CE / "manifest.json").read_text())
    assert ce.get("dec_060_started") is True
    assert ce.get("evo_000_started") is False
    # Docs cite the eight-item independent count
    bench = BENCH_DOC.read_text()
    assert "DEC-060" in bench
    assert "8" in bench or "eight" in bench.lower() or "首批8" in bench
    ref = REF.read_text()
    assert "DEC-060" in ref
    return True


def ac2_offline_cli_mcp_hard_reject_killswitch_perf() -> bool:
    offline = json.loads((PACK / "offline-path.json").read_text())
    budget = json.loads((PACK / "budget-crosscheck.json").read_text())
    perf_raw = json.loads((PACK / "perf-raw" / "offline-parse-hash-samples.json").read_text())

    assert offline["no_model_config"]["assessment_path_imports_model_client"] is False
    flags = offline["no_model_config"]["replay_flags"]
    assert flags["reread_production_state"] is False
    assert flags["reran_tools"] is False
    assert flags["model_or_network_requests"] is False
    assert offline["cli_mcp_parity"]["shared_assessment_hash"] is True
    assert offline["hard_reject"]["critical_families_zero_tolerance"] is True
    assert offline["hard_reject"]["offset_by_average_rejected"] is True
    # Re-execute the compare harness. A handwritten flag is not the check.
    sys.path.insert(0, str(HERE))
    import compare as compare_mod

    contract = json.loads((HERE / "contract.json").read_text())
    budgets = json.loads((HERE / "budgets.json").read_text())
    samples = json.loads((HERE / "sample-receipts.json").read_text())
    bad = compare_mod.compare_pair(
        contract,
        budgets,
        samples["baseline_receipt"],
        samples["candidate_receipt_hard_fail_offset_attempt"],
    )
    assert bad["passed"] is False
    assert bad["families"]["structural_correctness"]["hard_pass"] is False
    assert bad["families"]["structural_correctness"].get("offset_attempt_rejected") is True
    assert offline["kill_switch_fallback"]["advice_mode_disabled_restores_prior_advice"] is True
    assert offline["kill_switch_fallback"]["hard_protections_retained"] is True

    # Frozen replay fixtures still present and offline-shaped
    baseline = json.loads((REPLAY / "baseline-snapshot.json").read_text())
    assert baseline["schema_id"] == "awr-assessment-replay-snapshot-v1"
    assert baseline["rule_hash"]
    assert (REPLAY / "candidate-snapshot.json").is_file()

    # Perf raw retained; no ROI invention; budgets cross-checked
    assert len(perf_raw["samples"]) >= 30
    assert perf_raw["environment"]["offline"] is True
    assert perf_raw["environment"]["model_configured"] is False
    assert perf_raw["environment"]["network_calls_intended"] is False
    assert all(s["model_or_network_requests"] is False for s in perf_raw["samples"])
    assert "fabricate_ms_improvement" in perf_raw["forbid"]
    assert "derive_dollar_savings" in perf_raw["forbid"]
    assert budget["roi_claims_invented"] is False
    assert budget["dollar_savings_derived"] is False
    assert budget["ms_improvement_fabricated"] is False
    # Refuse later-version candidacy while budgets unbound
    if not budget["baseline_bound"]:
        assert "do_not_label_later_version_candidate" in budget["decision"]

    # Nested proves still pass (reuse, do not rebuild)
    for script in (
        CE / "verify.py",
        HERE / "verify.py",
        HERE / "prove_acceptance.py",
        HERE / "prove_acceptance_dec022.py",
        ROOT / "tests" / "fixtures" / "assessment" / "envelope" / "verify.py",
        ROOT / "tests" / "fixtures" / "assessment" / "signals" / "verify.py",
        ROOT / "tests" / "fixtures" / "assessment" / "contracts" / "verify.py",
    ):
        _run(script)
    return True


def ac3_explicit_non_claims() -> bool:
    non = json.loads((PACK / "non-claims.json").read_text())
    follow = json.loads((PACK / "follow-ons.json").read_text())
    manifest = json.loads((PACK / "manifest.json").read_text())

    ids = {d["id"] for d in non["does_not_prove"]}
    assert ids == REQUIRED_NON_CLAIMS
    assert non["explicit_test_assertions_required"] is True
    # Manifest also lists separation
    for key in (
        "full_14_item_suite",
        "AUTO",
        "Team",
        "native_host",
        "released_version",
    ):
        assert key in manifest["count_separately_from"]

    # Docs must state non-claims (Chinese acceptance AC3 or English equivalents)
    bench = BENCH_DOC.read_text()
    ref = REF.read_text()
    blob = bench + "\n" + ref
    assert "不代证" in blob or "does not prove" in blob.lower() or "non-claim" in blob.lower()
    for needle in ("14", "AUTO", "Team"):
        assert needle in blob
    # Gate must not have started follow-on tracks
    assert follow["dec_040_started"] is False
    assert follow["dec_041_started"] is False
    assert follow["evo_000_started"] is False
    assert manifest["dec_040_started"] is False
    assert manifest["evo_000_started"] is False

    # Guard: reject positive over-claims. Negated forms (不代证 / does not prove) are required.
    overclaim_patterns = [
        r"(?<![Nn]ot\s)(?<!does\snot\s)proves the entire 14-item",
        r"(?<![Nn]ot\s)(?<!does\snot\s)proves AUTO",
        r"(?<![Nn]ot\s)(?<!does\snot\s)proves Team",
        r"(?<![Nn]ot\s)(?<!does\snot\s)proves native host",
        r"(?<![Nn]ot\s)(?<!does\snot\s)proves a released",
        r"(?<!不)代证整个14项",
        r"(?<!不)代证\s*AUTO",
        r"(?<!不)代证\s*Team",
    ]
    for pat in overclaim_patterns:
        m = re.search(pat, blob, flags=re.IGNORECASE)
        assert m is None, f"forbidden over-claim present: {pat} -> {m.group(0)!r}"
    return True


def main() -> int:
    a1 = ac1_cross_check_eight_independent()
    a2 = ac2_offline_cli_mcp_hard_reject_killswitch_perf()
    a3 = ac3_explicit_non_claims()
    print(
        json.dumps(
            {
                "work": "AWR-DEC-060",
                "acceptance": {
                    "1_cross_check_010_through_022_eight_items_independent": a1,
                    "2_offline_nomodel_cli_mcp_hard_reject_killswitch_perf_raw": a2,
                    "3_proves_current_offline_explain_only_explicit_non_claims": a3,
                },
                "first_batch_count": 8,
                "dec_040_started": False,
                "dec_041_started": False,
                "evo_000_started": False,
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001
        print(f"FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1)
