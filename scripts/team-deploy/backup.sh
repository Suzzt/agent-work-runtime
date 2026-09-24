#!/usr/bin/env bash
# Owner logical backup manifest create/inspect (TMCP-041).
set -euo pipefail

AWR_SERVER="${AWR_SERVER:-awr-server}"
ACTION="create"
TENANT=""
PROJECT=""
BACKUP_ID=""

usage() {
  cat >&2 <<USAGE
Usage:
  $0 create --tenant-id <t> --project-id <p> [--awr-server <path>]
  $0 inspect --tenant-id <t> --project-id <p> --backup-id <id> [--awr-server <path>]
Requires AWR_TEAM_DATABASE_URL as schema owner. Not reachable from member MCP.
USAGE
  exit 2
}

[[ $# -ge 1 ]] || usage
ACTION="$1"; shift

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tenant-id) TENANT="${2:-}"; shift 2 ;;
    --project-id) PROJECT="${2:-}"; shift 2 ;;
    --backup-id) BACKUP_ID="${2:-}"; shift 2 ;;
    --awr-server) AWR_SERVER="${2:-}"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

[[ -n "${AWR_TEAM_DATABASE_URL:-}" ]] || {
  echo "AWR_TEAM_DATABASE_URL is required (owner connection)" >&2
  exit 1
}
[[ -n "$TENANT" && -n "$PROJECT" ]] || usage

case "$ACTION" in
  create)
    exec "$AWR_SERVER" access backup-create --tenant-id "$TENANT" --project-id "$PROJECT"
    ;;
  inspect)
    [[ -n "$BACKUP_ID" ]] || usage
    exec "$AWR_SERVER" access backup-inspect \
      --tenant-id "$TENANT" --project-id "$PROJECT" --backup-id "$BACKUP_ID"
    ;;
  *)
    usage
    ;;
esac
