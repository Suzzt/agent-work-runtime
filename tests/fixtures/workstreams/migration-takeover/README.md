# Migration takeover fixtures (WS-050)

Independent realistic fixture used to prove **backup → preview → migrate → restore**
before any in-repo project mainline takeover evidence is accepted.

Schema: `awr-workstream-migration-takeover-fixture/v1`

Coverage:
- Goal lines Team / EVO / DEC / AUTO with person owners
- Actors: persons + agents (provable and unprovable person-delegation)
- Sessions, claims, reviews (including an Agent historically declared as
  `independent_approver`)
- Incomplete work, an active session, and historical release evidence

Rules exercised by `awr_core::migration_takeover`:
- Preserve original actor / session / claim / review identities
- Mark unprovable person-delegation history as `pending_confirmation`
- Never auto-promote an Agent to owner or independent approver
- Require the independent fixture drill before project takeover dry-run
