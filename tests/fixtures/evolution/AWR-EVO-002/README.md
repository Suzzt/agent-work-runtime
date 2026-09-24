# AWR-EVO-002 evidence mirror

Checked-in mirror for Team-compatible local planning validation.

Authoritative local working copies (gitignored):

- `.local/awr-evolution-20260919/plan-validator-compat/`
- `scripts/check_ledger.py` (local wrapper; gitignored)
- `ledger/README.md` (local derived index notes; gitignored)
- `.local/awr-evo-002/` claim/session/design/evidence

## Layout

- `contracts/` — synthetic `awr-v1`, `awr-team-v1`, `awr-team-acceptance-v1`
- `repro/` — fixed reproductions of prep-period `FAIL: 'work_items'` and misplaced `AWR-G-TEAM`
- `positive/` — multi-contract books that pass type+version discrimination
- `negative/` — wrong binding, unknown type/version, duplicate scope, forged completion

## Validate

```sh
python3 scripts/evolution/verify_evo_002_plan_validator.py
python3 scripts/check_public_tree.py
```

Planning validation only. Does not rewrite V1/Team denominators. Does not start
EVO-010 / DEC-040 / DEC-041. Product completion still requires real runtime evidence.
