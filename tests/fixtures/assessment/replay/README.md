# Assessment replay fixtures (DEC-022)

Snapshots are artifact-shaped JSON (`awr-assessment-replay-snapshot-v1`) suitable for
storage via existing event/artifact stores. They freeze prepared facts + policy/rule
hashes so offline replay never re-reads production state, re-runs tools, or calls models.

Generated/verified by `crates/awr-runtime/tests/assessment_offline.rs`.
