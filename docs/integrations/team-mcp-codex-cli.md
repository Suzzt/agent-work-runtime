# Team MCP workflow · Codex (`codex_cli`) — AWR-TMCP-041

> **Remote Team MCP**, not personal stdio `awr-mcp`. Do not treat
> `awr team command` placeholders as a live remote transport
> ([team-access.md](../reference/team-access.md)).
>
> WS-024 adapter id: [`codex_cli`](named-agent-host.md). Personal Codex L2 notes
> remain in [codex.md](codex.md); this page is the **Team service** natural path.

## Prerequisites

From the operator: [member handoff](../reference/team-member-handoff.md) —
HTTPS MCP URL, personal bearer claim, and repo. Clone the repo for code work;
coordination goes through Team MCP.

Merge the remote MCP template
[`examples/team-mcp-deploy/clients/codex_cli.mcp.toml.example`](../../examples/team-mcp-deploy/clients/codex_cli.mcp.toml.example)
into the trusted project's `.codex/config.toml` (or register with `codex mcp`).
Load the bearer from the environment — never commit it.

Reload Codex and confirm `/mcp` shows the Team server connected.

## Tools

| Tool | Use |
| --- | --- |
| `awr_team_query` | Reads: `capabilities`, `work.list` / `search` / `prepare`, `claim.inspect`, `session.inspect`, `planning.outcome`, … |
| `awr_team_command` | Writes: `session.*`, `claim.*`, `execution.*`, `evidence.*`, `review.*`, `delivery.*`, `work.rework` / `work.complete` |

Stable `request_id` on every command. On disconnect, inspect the **same**
`request_id` before minting a new one.

## Natural workflow

### 1. Query

```json
{"protocol_version":1,"op":"capabilities"}
```

```json
{"protocol_version":1,"op":"work.list","args":{"workstream_id":"<ws>","limit":20}}
```

```json
{"protocol_version":1,"op":"work.search","args":{"workstream_id":"<ws>","search":"<literal>","limit":20}}
```

Pick a claimable work id from the authorized list only.

### 2. Claim / renew

Start a durable session, then acquire a claim (names are Team ops, not personal
CLI flags):

```json
{
  "protocol_version": 1,
  "request_id": "<stable-uuid>",
  "op": "session.start",
  "args": {"conversation_id": "codex:<native-thread-id>"}
}
```

```json
{
  "protocol_version": 1,
  "request_id": "<stable-uuid>",
  "op": "claim.acquire",
  "args": {
    "session_id": "<from start>",
    "expected_session_version": "<n>",
    "expected_work_version": "<n>",
    "ttl_seconds": 3600
  }
}
```

Renew before expiry with `claim.renew` (fence + lease version from the receipt).
Use `claim.inspect` for **current** lease state — replay of `claim.acquire` is a
historical receipt, not a renew.

### 3. Context

```json
{
  "protocol_version": 1,
  "op": "work.prepare",
  "args": {"work_id": "<work>", "max_context_bytes": 120000}
}
```

Consume the returned contract / `required_specs` / `authorized_readable_refs`
before mutating. Controlled body reads use `source.content` /
`artifact.content` — never invent server paths.

### 4. Checkpoint

After real progress:

```json
{
  "protocol_version": 1,
  "request_id": "<stable-uuid>",
  "op": "session.checkpoint",
  "args": {
    "session_id": "<id>",
    "expected_session_version": "<n>",
    "context_hash": "<hash from prepare you actually consumed>",
    "next_action": "Implement filtered query API",
    "open_loops": ["Await schema review"]
  }
}
```

MCP disconnect ≠ session end. Exit with `session.end` only when finished or
handing off.

### 5. PR link

Open the GitHub PR from the repo checkout, then register facts on Team MCP
([pr-delivery-review.md](pr-delivery-review.md)):

```json
{
  "protocol_version": 1,
  "request_id": "<stable-uuid>",
  "op": "delivery.register_pr",
  "args": {
    "repository": "originoneai/example",
    "pr_number": 1,
    "pr_url": "https://github.com/originoneai/example/pull/1",
    "head_sha": "<40-char lowercase hex>",
    "fact_source": "authorized_human_github_verification",
    "observed_at": "<RFC3339>"
  }
}
```

A URL alone is not acceptance. Submit evidence / request review with
`evidence.submit`, `review.open`, `delivery.submit_and_request_review` as
authorized.

### 6. Rework

When review returns the round, acknowledge with `work.rework` (author
acknowledgment — not independent review). Fix code, push a new head, then
`delivery.observe_pr` with `expected_head_sha` and re-open review as needed.
Old approvals bound to a prior head/contract invalidate.

### 7. Complete

Independent reviewer calls `review.accept` / `review.decide` under
`independent_review`. Maintainer finalizes with `delivery.finalize` /
`work.complete`. Green CI, admin role, or GitHub merge **never** skip AWR
acceptance.

## Host adapter note

For WS-024 capability negotiation, adapter id is `codex_cli`. Coordination
admission is not process start/kill — see [named-agent-host.md](named-agent-host.md).
