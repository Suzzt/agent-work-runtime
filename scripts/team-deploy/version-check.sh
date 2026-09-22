#!/usr/bin/env bash
# Verify Team schema matches the deployed awr-server binary (TMCP-041).
set -euo pipefail

AWR_SERVER="${AWR_SERVER:-awr-server}"

usage() {
  echo "Usage: $0 [--awr-server <path>]" >&2
  echo "Requires AWR_TEAM_DATABASE_URL (owner or app may check; migrate stays owner)." >&2
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --awr-server) AWR_SERVER="${2:-}"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

if [[ -z "${AWR_TEAM_DATABASE_URL:-}" ]]; then
  echo "AWR_TEAM_DATABASE_URL is required" >&2
  exit 1
fi

echo "Running: $AWR_SERVER check"
exec "$AWR_SERVER" check
