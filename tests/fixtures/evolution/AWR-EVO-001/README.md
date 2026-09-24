# AWR-EVO-001 evidence mirror

Checked-in mirror of the EVO-001 acceptance-matrix / baseline-workload / cost-auth freeze.

Authoritative local working copies (ignored by Git):

- `.local/awr-evolution-20260919/evaluation-plan.json`
- `.local/awr-evolution-20260919/fixture-manifest.json`
- `.local/awr-evolution-20260919/authorization-requirements.md`
- `.local/awr-evo-001/` claim/session/design/evidence

This item **formulates the experiment only**. It does not call paid models or start
native agents. Native/paid verification stay blocked until explicit authorization
fields are present (see `authorization-requirements.md`).

RET-AMD-001: independent R0/R1/R2 matrix for RET-006 is frozen; the old 36-trial
proposal/budget does not auto-expand.

Docs pointer: `docs/benchmarks/evolution-acceptance-matrix.md`

Validate from a clean checkout. The script reads this fixture. A private
`.local/awr-evolution-20260919` copy is optional; if present, it must match.

```sh
python3 scripts/evolution/verify_evo_001_plan.py
python3 scripts/check_public_tree.py
```
