# Workstream parallel / handoff business fixtures (AWR-WS-051)

Six `scope_revision=2` scenarios (`WS-BIZ-01` … `06`) with independent
namespaces, identities, work graphs, natural prompts, and honest `待验`
report templates.

See `docs/reference/workstream-parallel-handoff-biz.md`.

```sh
python3 tests/fixtures/workstreams/parallel-handoff-biz/run_harness.py
python3 -m unittest tests.workstream-parallel-handoff-biz.test_fixtures -v
```

Live multi-person / named-client / Team Web gates remain blocked until a real
trial team supplies evidence. Fixture presence alone is not completion.
