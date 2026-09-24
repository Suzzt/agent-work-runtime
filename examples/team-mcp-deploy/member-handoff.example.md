# Member handoff (fill in and send out of band)

1. **MCP address:** `https://team.example/v1/projects/example/mcp`
2. **Personal credential claim:** Operator will deliver a one-time bearer via
   `<encrypted channel>`. Store it in a local env var `AWR_TEAM_BEARER`.
   Do not paste it into Git, chat, or MCP tool arguments. Rotate by asking a
   project admin for a new credential id (old id revoked).
3. **Repository:** `https://github.com/example/org-repo` — contribute via PR.
   Do not expect DB accounts or server ledger-directory write access.

Client setup:
- Codex: `examples/team-mcp-deploy/clients/codex_cli.mcp.toml.example`
- Claude Code: `examples/team-mcp-deploy/clients/claude_code.mcp.json.example`

First call: `awr_team_query` with `{"protocol_version":1,"op":"capabilities"}`.
