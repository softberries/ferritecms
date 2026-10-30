#!/usr/bin/env bash
# The backward-compatibility check (M4-2 P38; design 02 §8, S5: an older binary keeps
# running against a newer schema). The NEW binary creates a site (`init` + `seed demo`),
# which migrates the database to its schema; then the OLD binary must report it
# compatible (`migrate status` exit 0), serve it, and answer `/` and `/healthz` with 200.
#
#   .github/scripts/compat.sh OLD_BINARY NEW_BINARY WORK_DIR [POSTGRES_URL]
#
# Always checks SQLite; with POSTGRES_URL also Postgres (in the schema `ferrite_compat`,
# which must not exist yet). WORK_DIR must not exist or be empty.
set -euo pipefail

if [ "$#" -lt 3 ] || [ "$#" -gt 4 ]; then
  echo "usage: $0 OLD_BINARY NEW_BINARY WORK_DIR [POSTGRES_URL]" >&2
  exit 1
fi
old=$1
new=$2
work=$3
pg_url=${4:-}
mkdir -p "$work"

serve_pid=""
cleanup() {
  if [ -n "$serve_pid" ] && kill -0 "$serve_pid" 2>/dev/null; then
    kill -TERM "$serve_pid" 2>/dev/null || true
    wait "$serve_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo "old: $("$old" --version)"
echo "new: $("$new" --version)"

# check NAME PORT [init args...]
check() {
  local name=$1 port=$2
  shift 2
  local data="$work/$name"
  local log="$work/$name-serve.log"
  local base="http://127.0.0.1:$port"
  echo "== $name: the new binary creates and migrates the site"
  "$new" init --data "$data" --url "$base" "$@"
  "$new" seed demo --data "$data"
  "$new" migrate status --data "$data"

  echo "== $name: the old binary reads the newer schema"
  "$old" migrate status --data "$data"

  echo "== $name: the old binary serves it"
  FERRITE_SERVER__LISTEN="127.0.0.1:$port" "$old" serve --data "$data" >"$log" 2>&1 &
  serve_pid=$!
  local up=""
  for _ in $(seq 1 120); do
    if ! kill -0 "$serve_pid" 2>/dev/null; then
      break
    fi
    if grep -q '^Listening on ' "$log"; then
      up=1
      break
    fi
    sleep 0.5
  done
  if [ -z "$up" ]; then
    echo "the old binary did not start serving; its output:" >&2
    cat "$log" >&2
    exit 1
  fi
  for path in / /healthz; do
    local code
    code=$(curl -sS -o /dev/null -w '%{http_code}' "$base$path")
    echo "GET $path -> $code"
    if [ "$code" != 200 ]; then
      echo "expected 200 from $path; the server's output:" >&2
      cat "$log" >&2
      exit 1
    fi
  done
  kill -TERM "$serve_pid"
  local status=0
  wait "$serve_pid" || status=$?
  serve_pid=""
  if [ "$status" -ne 0 ]; then
    echo "the old binary's serve exited $status after SIGTERM; its output:" >&2
    cat "$log" >&2
    exit 1
  fi
  echo "== $name: ok"
}

check sqlite 18081
if [ -n "$pg_url" ]; then
  check postgres 18082 --db "$pg_url" --schema ferrite_compat
fi
echo "compat: ok"
