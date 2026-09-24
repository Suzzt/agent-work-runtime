# AWR-EVO-010 evidence mirror

Checked-in mirror of the five-layer result semantics and two host-integration
contracts freeze.

Authoritative local working copies (ignored by Git):

- `.local/awr-evolution-20260919/semantic-contract-matrix.json`
- `.local/awr-evo-010/` claim/session/design/evidence

## Layout

- `semantic-contract-matrix.json` — frozen matrix (byte-identical to local copy)
- `counterexamples/` — independent counterexamples for each layer, both host modes,
  separations, entry mixing, and compatibility
- `pointer.json` — hash/docs/verifier pointers

## Validate

Validate from a clean checkout. The script reads this fixture. A private
`.local/awr-evolution-20260919` copy is optional; if present, it must match.

```sh
python3 scripts/evolution/verify_evo_010_semantic_contract.py
python3 scripts/check_public_tree.py
```

Contract-first freeze only. Does not start EVO-011 / DEC-040 / DEC-041. Does not
require paid or native agent runs. Component mode must not hold a second work state.
