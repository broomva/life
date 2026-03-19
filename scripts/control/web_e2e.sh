#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

# Allow override for custom environments
if [ -n "${CONTROL_WEB_E2E_CMD:-}" ]; then
  eval "$CONTROL_WEB_E2E_CMD"
  exit 0
fi

# If Playwright is configured and a base URL exists, use Playwright
if [ -f playwright.config.ts ] && command -v npx >/dev/null 2>&1; then
  base_url="${APP_BASE_URL:-${PLAYWRIGHT_BASE_URL:-}}"
  if [ -n "$base_url" ]; then
    npx playwright test tests/e2e/web --reporter=line
    exit 0
  fi
fi

# Default: build arcand, start it, exercise canonical session API, tear down
passed=0
failed=0
server_pid=""

ok()   { echo "[PASS] $1"; passed=$((passed + 1)); }
fail() { echo "[FAIL] $1"; failed=$((failed + 1)); }

cleanup() {
  if [ -n "$server_pid" ] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  [ -n "${tmpdir:-}" ] && rm -rf "$tmpdir"
}
trap cleanup EXIT

echo "Web E2E: Testing arcand canonical HTTP API"
echo

# Build arcand
if [ -f arcan/Cargo.toml ]; then
  echo "--- Building arcand ---"
  if (cd arcan && cargo build -p arcan --quiet 2>/dev/null); then
    ok "arcand builds"
  else
    fail "arcand build"
    echo "Web E2E: Cannot continue without arcand binary."
    exit 1
  fi
fi

arcan_bin=""
for candidate in "arcan/.target/debug/arcan" "arcan/target/debug/arcan" ".target/debug/arcan" "target/debug/arcan"; do
  if [ -x "$candidate" ]; then
    arcan_bin="$candidate"
    break
  fi
done
if [ -z "$arcan_bin" ]; then
  echo "Web E2E: arcan binary not found in expected locations" >&2
  exit 1
fi

# Start arcand on a random port with temp storage
tmpdir=$(mktemp -d)
port=0

# Find an available port
if command -v python3 >/dev/null 2>&1; then
  port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()')
elif command -v python >/dev/null 2>&1; then
  port=$(python -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()')
else
  port=13579
fi

echo "--- Starting arcand on port $port ---"
LAGO_DATA_DIR="$tmpdir" "$arcan_bin" --port "$port" &
server_pid=$!

# Wait for server to be ready (up to 15s)
ready=false
for i in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
    ready=true
    break
  fi
  sleep 0.5
done

if ! $ready; then
  # Try without /health — maybe it doesn't have that endpoint
  if curl -fsS "http://127.0.0.1:$port/" >/dev/null 2>&1; then
    ready=true
  fi
fi

if ! $ready; then
  fail "arcand startup (not reachable after 15s)"
  echo "Web E2E: Server did not start. Check arcan binary."
  exit 1
fi
ok "arcand started on port $port"

base="http://127.0.0.1:$port"

echo
echo "--- Exercising canonical session API ---"

# POST /sessions — create a session
session_response=$(curl -fsS -X POST "$base/sessions" \
  -H "Content-Type: application/json" \
  -d '{}' 2>&1) && ok "POST /sessions" || fail "POST /sessions"

# Extract session_id if possible
session_id=""
if command -v jq >/dev/null 2>&1 && [ -n "$session_response" ]; then
  session_id=$(echo "$session_response" | jq -r '.session_id // .id // empty' 2>/dev/null || true)
fi

if [ -n "$session_id" ]; then
  # GET /sessions/{id}/state
  curl -fsS "$base/sessions/$session_id/state" >/dev/null 2>&1 \
    && ok "GET /sessions/$session_id/state" \
    || fail "GET /sessions/$session_id/state"

  # GET /sessions/{id}/events
  curl -fsS "$base/sessions/$session_id/events" >/dev/null 2>&1 \
    && ok "GET /sessions/$session_id/events" \
    || fail "GET /sessions/$session_id/events"
else
  echo "  (skipping session-specific endpoints — no session_id extracted)"
fi

echo

# --- Summary ---
total=$((passed + failed))
echo "Web E2E: $passed/$total passed"

if [ "$failed" -gt 0 ]; then
  echo "Web E2E: $failed test(s) failed."
  exit 1
fi

echo "Web E2E: All tests passed."
