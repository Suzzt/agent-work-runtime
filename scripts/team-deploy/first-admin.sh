#!/usr/bin/env bash
# Bootstrap the first project admin via owner access apply (TMCP-041).
# Daily member changes after this use project-admin MCP (TMCP-012), not this script.
set -euo pipefail

PLAN=""
REQUEST_ID=""
EXPECTED_STATE=""
EXPECTED_PLAN=""
PREVIEW_ONLY=0
AWR_SERVER="${AWR_SERVER:-awr-server}"

usage() {
  cat >&2 <<USAGE
Usage:
  $0 --plan <access-apply.json> [--preview-only]
  $0 --plan <access-apply.json> --request-id <id> \\
       --expected-state <digest> --expected-plan <digest>

Plan shape: docs/reference/team-operator-access.md (owner apply).
Requires AWR_TEAM_DATABASE_URL as schema owner.
Apply needs digests from a just-reviewed \`access preview\` of the same plan.
USAGE
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --plan) PLAN="${2:-}"; shift 2 ;;
    --request-id) REQUEST_ID="${2:-}"; shift 2 ;;
    --expected-state) EXPECTED_STATE="${2:-}"; shift 2 ;;
    --expected-plan) EXPECTED_PLAN="${2:-}"; shift 2 ;;
    --preview-only) PREVIEW_ONLY=1; shift ;;
    --awr-server) AWR_SERVER="${2:-}"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

[[ -n "$PLAN" && -f "$PLAN" ]] || usage
[[ -n "${AWR_TEAM_DATABASE_URL:-}" ]] || {
  echo "AWR_TEAM_DATABASE_URL is required (owner connection)" >&2
  exit 1
}

echo "Previewing first-admin plan: $PLAN"
"$AWR_SERVER" access preview --input "$PLAN"

if [[ "$PREVIEW_ONLY" -eq 1 ]]; then
  echo "preview-only: not applying"
  exit 0
fi

[[ -n "$REQUEST_ID" && -n "$EXPECTED_STATE" && -n "$EXPECTED_PLAN" ]] || {
  echo "apply requires --request-id --expected-state --expected-plan (from preview)" >&2
  echo "or pass --preview-only" >&2
  exit 2
}

echo "Applying first-admin plan (request_id=$REQUEST_ID)"
exec "$AWR_SERVER" access apply \
  --input "$PLAN" \
  --request-id "$REQUEST_ID" \
  --expected-state "$EXPECTED_STATE" \
  --expected-plan "$EXPECTED_PLAN"
