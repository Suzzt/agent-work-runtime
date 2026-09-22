# AWR-EVO-000 · Overlap and authority对照

Recorded: 2026-09-23 06:53:31 UTC+08:00 (UTC+8)
Worktree: `/workspace/awr-evo000-wt`
Branch: `codex/awr-evo-000-execution-baseline`
Construction HEAD: `41d2b97446b70c1b589289980853283b9f0a3674`
Construction tree: `2664e63847196371d315e7dacf3d4c021322d7dd`
Note: HEAD/tree above are the inventoried DEC-060 tip (stacked base). PR/`evidence.json` head_sha is the commit that adds this mirror.
Baseline JSON: `.local/awr-evolution-20260919/execution-baseline.json`
Checked-in mirror: `tests/fixtures/evolution/AWR-EVO-000/`

This document is an inventory/governance gate. It does **not** rewrite V1 / Team / business-acceptance denominators or historical records. Missing Mac-authoritative facts are marked **unknown**.

## 1. Unique work authority (两种接入模式)

| Mode | Fact owner (唯一事实所有者) | Transport | Non-negotiable boundary |
| --- | --- | --- | --- |
| **Personal / local** | Authoritative project sources (especially `ledger/work-ledger.yaml` and mapped Markdown/YAML) + personal SQLite projection under `.awr/`. Git merge ≠ completion. | `awr` CLI / personal `awr-mcp` | Do not silently replace sources with projections; do not init a second project for this special. |
| **Team / PG** | Team PostgreSQL coordination store + activated source contract snapshots under authenticated Team entry. | Team HTTP/MCP / `awr-server` / `awr team` | Missing/offline Team remote **must not** fall back to personal SQLite authority; personal MCP is not Team command dispatch. |

**本专项 identity:** keep original project `01M1YJBR5PW6QXYGABADJVAJPC` and status authority `ledger/work-ledger.yaml` (Mac path `file:///Users/mac/Documents/originone/agent-work-running/ledger/work-ledger.yaml`). Do **not** initialize a new project, delete `.awr`, or replace program bindings.

Evidence: `docs/reference/team-access.md`, `docs/TAKEOVER.md`, `AGENTS.md`, `.gitignore` private-path policy, work-show `source_ref` / `project_id`.

## 2. Requirement对照 (已有 / 需补 / 需复核)

Status vocabulary:
- **已有 (reuse)**: implemented capability or frozen public evidence to reuse; do not rebuild.
- **需补**: must be produced by this or a later authorized EVO task.
- **需复核**: exists but must be re-checked against current HEAD/contract before treating as current construction fact.
- **unknown**: not observable from this box; do not invent.

| ID | Requirement | 已有 | 需补 | 需复核 | Evidence / notes |
| --- | --- | --- | --- | --- | --- |
| EVO-000-AC1 | Baseline includes actual HEAD/tree, program source/version/summary, source fingerprints, active items, unknown executions; missing → unknown | Partial public program identity on tip | Box-local baseline JSON + mirror (this task) | Mac live ledger bytes & `.awr/project.toml` binding | `execution-baseline.json`; HEAD/tree recorded; Mac paths unknown |
| EVO-000-AC2 | Each requirement states 已有/需补/需复核 + evidence; keep old V1/Team/biz denominators | Team V1 matrix counts `required=69`, `real_agent_accepted=42`, historical index sealed; assessment DEC-010..060 public docs/fixtures | Full Mac evolution acceptance catalog text (not on box) | Whether Mac ledger still matches work-show fingerprint `sha256:215ec4d4…` | `docs/reference/team-v1-*.json`; this table; **denominators not rewritten** |
| EVO-000-AC3 | Two access modes each have unique fact owner; special keeps original project + `ledger/work-ledger.yaml` | Documented personal vs Team owners in public refs | none for identity policy | Mac `.awr/project.toml` exact binding contents | §1 above; work-show project_id |
| EVO-000-AC4 | On occupancy/permission/unknown-effect conflict: no conflicting write; state recovery + next legal action | Path occupancy for EVO deliverables free on box | Mac live writer inventory for ledger/RULES | Concurrent Mac edits to ledger | §4 below |
| REL-UX-001 | `AWR-UX-001` inventory only | unknown on box | none by EVO-000 | Mac owner/status | No claim copy under `/workspace/*/.local`; **do not modify owner/status** |
| REL-TEAM-P17 | `TEAM-P17` inventory only | Mentioned as still-active in EVO-000 work summary at work-show | none by EVO-000 | Mac owner/status/path set | work-show summary excerpt; **do not modify** |
| REL-QA-003 | `QA-003` inventory only | unknown on box | none by EVO-000 | Mac owner/status | No claim copy on box; **do not modify** |
| REUSE-DEC-010..060 | First-batch offline assessment/explain stack | Public stacked PRs #146–#153 + fixtures/tests on tip `41d2b974…` | EVO must not rebuild envelope/explain/offline gate | Mac `awr work complete` for DEC sessions still open on box copies | DEC worktrees + `docs/reference/assessment.md` |
| REUSE-TEAM-V1 | Team V1 scenario denominators & historical agent evidence | `team-v1-evidence-matrix.json` counts; historical index | none (preserve) | Matrix `last_run` still null for many cases (known limits) | known-limits.md; **do not rewrite counts/records** |
| REUSE-PERSONAL-RUNTIME | Personal CLI/MCP source-first runtime | In-tree crates + docs | none for EVO-000 gate | Box lacks Mac `.awr` binding | README 0.4.0; mcp-service.md |
| REUSE-WS/TMCP-STACKS | Open WS/TMCP stacked PRs on separate branches | Many open PRs #113–#145 (REST inventory) | none by EVO-000 | Live Mac claim vs PR completion drift | open PR list in baseline JSON; do not hijack those branches |
| MAC-RULES | `docs/RULES.md` | unknown (absent on box) | none here | Read when Mac mount/session available | gitignored |
| MAC-DESIGN-EVO | Evolution foundation design/acceptance/handoff | Partial: `.local/awr-evo-000/design/AWR-EVO-000.md` copy | Full catalog on Mac for later EVO tasks | Contract ids / acceptance denominators for whole EVO program | Mac paths listed in design §来源 |
| DEC-040/041 | Follow-on assessment work | Explicitly **not started** | Out of scope | n/a | `dec_040_started=false`, `dec_041_started=false` |

### Preserved denominators (explicit non-rewrite)

From `docs/reference/team-v1-evidence-matrix.json` at recording time:

- `required`: 69
- `protocol_implemented`: 67
- `real_agent_accepted`: 42
- `automated_partial`: 69
- `automated_evidence_pending`: 27

Historical agent evidence remains in `docs/reference/team-v1-historical-agent-evidence-v1.json` (sealed index; `historical_pending_review` semantics per known-limits). EVO-000 does **not** claim these were revalidated or rewritten.

## 3. Active items & unknown executions (summary)

### Observable on this shared box

- **13** claim/session copies under `/workspace/*/.local/*/{claim,session-start}.json` for project `01M1YJBR5PW6QXYGABADJVAJPC`, including this EVO-000 session `01M35N0C8F0TFFMM9KPZ1P7JZV` / claim `01M35N0C8HNW4DN00P3ZBB8QTG`.
- **44** git worktrees attached to the shared repo object store (DEC/WS/TMCP/CR stacks + main checkout).
- **41** open PRs on `originoneai/awr` (REST), including DEC stack #146–#153 and WS/TMCP stacks. Base for this stacked PR: `codex/awr-dec-060-mvp-offline-explain-gate` (#153).

### Unknown (Mac AWR live)

- Live claim/session/execution registry beyond copied JSON.
- Live owner/status/path occupancy for `AWR-UX-001`, `TEAM-P17`, `QA-003`.
- Live `ledger/work-ledger.yaml` bytes (fingerprint at work-show only).
- `.awr/project.toml` runtime binding contents.
- Whether DEC/WS/TMCP Mac claims were released after PR open (`mac_work_complete` false in DEC-060 local completion evidence).

Unknown executions are recorded as **unknown**, not guessed clear.

## 4. Path occupancy / conflict policy

| Path / resource | Occupant | Conflict? | Recovery / next legal action |
| --- | --- | --- | --- |
| `.local/awr-evolution-20260919/*` | EVO-000 | No | Continue writing baseline artifacts |
| `tests/fixtures/evolution/AWR-EVO-000/*` | EVO-000 (checked-in mirror) | No | Commit + stacked PR |
| `ledger/evidence/AWR-EVO-000/` | Mac-authoritative when present; absent on box | No box conflict | Parent/Mac may copy mirror receipts into Mac ledger evidence; **box must not fabricate Mac ledger authority** |
| `ledger/work-ledger.yaml` | Mac source authority | Effect-unknown from box | **Do not edit** from this agent; if Mac shows foreign active writer on EVO rows, wait or explicit handoff |
| `.awr/` | Mac personal runtime | Absent on box | **Do not init / delete / replace bindings** |
| `codex/awr-evo-000-execution-baseline` | EVO-000 | No | Push + open PR against DEC-060 branch |
| DEC-060 tip files | DEC-060 owner | Shared tip SHA only | Read-only base; no DEC-040/041 start; no mutation of DEC acceptance fixtures except EVO mirror paths |
| Other stacked branches (WS/TMCP/DEC) | Their agents/PRs | Orthogonal paths | Do not push to their branches or alter their owner/status |

If a conflict is discovered (foreign claim on EVO deliverable paths, permission denial, or unknown-effect writer on Mac ledger for this work):

1. **Stop** conflicting writes immediately.
2. Record blocker + recovery condition in local evidence (and Mac ledger when authorized).
3. Next legal actions: wait for release, request explicit handoff, or re-prepare/claim only after source refresh shows clear occupancy — never steal claim or rewrite foreign owner/status.

## 5. Checks used for this gate

- `python3 scripts/check_public_tree.py`
- `python3 scripts/evolution/verify_evo_000_baseline.py` (schema/occupancy/AC mapping)
- Read-only inventory: git HEAD/tree, worktree list, local claim copies, GitHub REST open PRs
- **Not run:** Mac `awr work complete`; DEC-040/041; global install/hook changes; new project `awr init`

## 6. Explicit non-claims

- Does not prove full evolution-foundation acceptance.
- Does not revalidate Team V1 business acceptance or historical agent runs.
- Does not assert Mac live claim registry beyond copied files.
- Does not start DEC-040, DEC-041, or other non-EVO-000 work.
