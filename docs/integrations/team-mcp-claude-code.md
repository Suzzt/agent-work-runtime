# Team MCP workflow · Claude Code (`claude_code`) — AWR-TMCP-041

> **Remote Team MCP**, not personal stdio `awr-mcp`. Do not treat
> `awr team command` placeholders as a live remote transport
> ([team-access.md](../reference/team-access.md)).
>
> WS-024 adapter id: [`claude_code`](named-agent-host.md) (not auto-startable).
> Capability summary: [claude-code-agent.md](claude-code-agent.md).

## Prerequisites

From the operator: [member handoff](../reference/team-member-handoff.md) —
HTTPS MCP URL, personal bearer claim, and repo.

Merge
[`examples/team-mcp-deploy/clients/claude_code.mcp.json.example`](../../examples/team-mcp-deploy/clients/claude_code.mcp.json.example)
into Claude Code's MCP config (user or project). Bearer via env / headers only.

Start Claude Code yourself (AWR will not launch it). Confirm the Team MCP server
lists `awr_team_query` and `awr_team_command`.

## Tools

Same Team surface as Codex:

| Tool | Use |
| --- | --- |
| `awr_team_query` | `capabilities`, work/session/claim reads, controlled content |
| `awr_team_command` | session / claim / execution / delivery / rework / complete |

Use a stable `request_id`. Prefer `command.inspect` / `planning.outcome` on
uncertainty before retrying with a new id.

## Natural workflow

### 1. Query

Call `awr_team_query` with `{"protocol_version":1,"op":"capabilities"}`, then
`work.list` / `work.search` on your authorized `workstream_id`. Only claim work
you can see.

### 2. Claim / renew

1. `session.start` with `conversation_id` like `claude:<native-session-id>`.
2. `claim.acquire` with session + expected work/session versions and TTL.
3. `claim.renew` on the active fence/lease before expiry.
4. `claim.inspect` when unsure — do not interpret an acquire replay as renew.

Bind external session labels with L0 habits from
[claude-code-agent.md](claude-code-agent.md) (`claude:<native-id>`), but perform
coordination on **Team** tools above.

### 3. Context

`work.prepare` for the claimed work. Read `required_specs` and
`authorized_readable_refs`. Fetch bodies only through `source.content` /
`artifact.content`. Recheck prepare after relevant source publishes.

### 4. Checkpoint

`session.checkpoint` with the **consumed** `context_hash`, concrete
`next_action`, and `open_loops`. Closing Claude Code or the MCP transport does
not end the durable Team session — use `session.end` or an authorized handoff.

### 5. PR link

Develop in your git checkout of the handed repo. After opening the PR:

- `delivery.register_pr` with repository, number, URL, `head_sha`,
  `fact_source`, `observed_at`
- `evidence.submit` / `review.open` / `delivery.submit_and_request_review`

See [pr-delivery-review.md](pr-delivery-review.md). GitHub UI state is not AWR
completion.

### 6. Rework

On `review.return`, author runs `work.rework`, fixes the head, notifies with
`delivery.observe_pr` (`expected_head_sha`), and requests a fresh review round.
Retained history keeps failed/rejected rounds.

### 7. Complete

Eligible independent person: `review.accept` / `review.decide`. Maintainer:
`delivery.finalize` / `work.complete`. Agents of the same person are not
team-independent reviewers.

## Host adapter note

`claude_code` supports status / reconnect / forensics; **start** and
**stop_confirmation** return human continuation. Prefer reconnecting the same
execution identity before starting anything new
([named-agent-host.md](named-agent-host.md)).
