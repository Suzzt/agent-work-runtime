#!/usr/bin/env bash
# Owner-only Team schema migrate + optional app-role grants (TMCP-041).
set -euo pipefail

APP_ROLE=""
AWR_SERVER="${AWR_SERVER:-awr-server}"

usage() {
  echo "Usage: $0 [--app-role <role>] [--awr-server <path>]" >&2
  echo "Requires AWR_TEAM_DATABASE_URL as the schema owner (ops shell)." >&2
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app-role) APP_ROLE="${2:-}"; shift 2 ;;
    --awr-server) AWR_SERVER="${2:-}"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

if [[ -z "${AWR_TEAM_DATABASE_URL:-}" ]]; then
  echo "AWR_TEAM_DATABASE_URL is required (owner connection)" >&2
  exit 1
fi

if [[ -n "$APP_ROLE" ]]; then
  exec "$AWR_SERVER" migrate --app-role "$APP_ROLE"
else
  exec "$AWR_SERVER" migrate
fi
