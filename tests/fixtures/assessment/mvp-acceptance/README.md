# First-batch MVP acceptance evidence pack (DEC-060)

Checked-in equivalent of the planned `.local/awr-decision-20260920/mvp-acceptance/`
directory (`.local/` must not ship).

## Contents

| File | Role |
| --- | --- |
| `manifest.json` | Eight first-batch items counted independently |
| `gate-checklist.json` | AC1 cross-check of DEC-010..022 acceptance receipts |
| `offline-path.json` | AC2 offline/no-model + CLI/MCP + hard-reject + kill-switch map |
| `non-claims.json` | AC3 explicit non-claims (14-suite / AUTO / Team / native host / release) |
| `budget-crosscheck.json` | Pre-registered budgets inspected; no ROI invention |
| `follow-ons.json` | Follow-on enhancements and scheduling conditions |
| `prior-acceptance/` | Slim independent re-check receipts for DEC-010..022 |
| `perf-raw/` | Retained performance raw samples |

## Re-run the gate

```sh
python3 tests/benchmarks/assessment/prove_acceptance_dec060.py
```

Optional fuller offline walk (also invoked by the prove script when `--full`):

```sh
python3 tests/benchmarks/assessment/run_offline_gate.py
```
