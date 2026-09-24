# AWR-EVO-000 evidence mirror

Checked-in mirror of the EVO-000 execution-baseline gate.

Authoritative local working copies (ignored by Git):

- `.local/awr-evolution-20260919/execution-baseline.json`
- `.local/awr-evolution-20260919/overlap-and-authority.md`
- `.local/awr-evo-000/` claim/session/design/evidence

Mac-authoritative ledger evidence path `ledger/evidence/AWR-EVO-000/` is not
present on the Linux box; this fixture is the repo-traceable mirror. It does
**not** rewrite Team V1 / business-acceptance denominators.

Validate from a clean checkout. The script reads this fixture. A private
`.local/awr-evolution-20260919` copy is optional; if present, it must match.

```sh
python3 scripts/evolution/verify_evo_000_baseline.py
python3 scripts/check_public_tree.py
```
