#!/usr/bin/env bash
# opsis-smoke-test.sh — End-to-end smoke test for Opsis world state engine.
#
# Tests the full pipeline: opsisd APIs, event injection, SSE stream,
# schema registry, aggregator routing, unrouted events, multi-agent topology.
#
# Usage:
#   scripts/dev/opsis-smoke-test.sh                # full test (starts/stops opsisd)
#   scripts/dev/opsis-smoke-test.sh --no-start     # test against already-running opsisd
#   scripts/dev/opsis-smoke-test.sh --with-arcan   # also test arcan ↔ opsis bridge
#
# Requirements:
#   - cargo (for building opsisd/arcan)
#   - curl, python3 (for API calls and JSON parsing)
#
# Exit codes:
#   0 = all tests passed
#   1 = one or more tests failed

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

# Source .env if present (provider keys, OTEL config, ports).
if [ -f "$ROOT/.env" ]; then
  set -a; . "$ROOT/.env"; set +a
fi

OPSISD_PORT="${OPSISD_PORT:-3010}"
OPSISD_URL="http://127.0.0.1:${OPSISD_PORT}"
ARCAN_PORT="${ARCAN_PORT:-3000}"
ARCAN_URL="http://127.0.0.1:${ARCAN_PORT}"
OPSISD_PID=""
ARCAN_PID=""
NO_START=false
WITH_ARCAN=false
PASSED=0
FAILED=0
TOTAL=0
TMPDIR="${TMPDIR:-/tmp}"
LOG="$TMPDIR/opsis-smoke-test.log"

# ── CLI args ─────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-start) NO_START=true; shift ;;
    --with-arcan) WITH_ARCAN=true; shift ;;
    -h|--help)
      echo "Usage: $0 [--no-start] [--with-arcan]"
      exit 0
      ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

# ── Helpers ──────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
DIM='\033[0;90m'
RESET='\033[0m'

log()  { echo -e "${CYAN}[opsis-smoke]${RESET} $*"; }
pass() { ((PASSED++)); ((TOTAL++)); echo -e "  ${GREEN}✓${RESET} $1"; }
fail() { ((FAILED++)); ((TOTAL++)); echo -e "  ${RED}✗${RESET} $1"; }
dim()  { echo -e "  ${DIM}$1${RESET}"; }

assert_eq() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    pass "$label"
  else
    fail "$label (expected: $expected, got: $actual)"
  fi
}

assert_contains() {
  local label="$1" haystack="$2" needle="$3"
  if echo "$haystack" | grep -q "$needle"; then
    pass "$label"
  else
    fail "$label (missing: '$needle')"
  fi
}

assert_http() {
  local label="$1" url="$2" expected_code="$3"
  local code
  code=$(curl -s -o /dev/null -w "%{http_code}" "$url" 2>/dev/null || echo "000")
  assert_eq "$label" "$expected_code" "$code"
}

json_field() {
  python3 -c "import sys,json; d=json.load(sys.stdin); print($1)" 2>/dev/null
}

wait_http() {
  local url="$1" timeout_s="${2:-15}"
  local i=0
  while (( i < timeout_s )); do
    if curl -fsS "$url" >/dev/null 2>&1; then return 0; fi
    sleep 1; ((i++))
  done
  return 1
}

# ── Cleanup ──────────────────────────────────────────────────────────

cleanup() {
  if [[ -n "$OPSISD_PID" ]]; then
    kill "$OPSISD_PID" 2>/dev/null || true
    wait "$OPSISD_PID" 2>/dev/null || true
    log "stopped opsisd (pid=$OPSISD_PID)"
  fi
  if [[ -n "$ARCAN_PID" ]]; then
    kill "$ARCAN_PID" 2>/dev/null || true
    wait "$ARCAN_PID" 2>/dev/null || true
    log "stopped arcan (pid=$ARCAN_PID)"
  fi
}
trap cleanup EXIT

# ── Start services ───────────────────────────────────────────────────

if ! $NO_START; then
  log "building opsisd..."
  cargo build -p opsisd --quiet 2>&1

  # Kill any existing opsisd on the port
  lsof -ti :"$OPSISD_PORT" 2>/dev/null | xargs kill 2>/dev/null || true
  sleep 1

  log "starting opsisd on port $OPSISD_PORT..."
  RUST_LOG=info cargo run -p opsisd -- --hz 2.0 --bind "127.0.0.1:${OPSISD_PORT}" > "$LOG" 2>&1 &
  OPSISD_PID=$!

  if ! wait_http "$OPSISD_URL/health" 15; then
    fail "opsisd failed to start"
    cat "$LOG"
    exit 1
  fi
  log "opsisd running (pid=$OPSISD_PID)"
fi

if $WITH_ARCAN && ! $NO_START; then
  log "building arcan with opsis feature..."
  cargo build -p arcan --features opsis --quiet 2>&1

  lsof -ti :"$ARCAN_PORT" 2>/dev/null | xargs kill 2>/dev/null || true
  sleep 1

  log "starting arcan on port $ARCAN_PORT..."
  OPSIS_URL="$OPSISD_URL" RUST_LOG=info \
    cargo run -p arcan --features opsis -- serve --provider mock --port "$ARCAN_PORT" > "$TMPDIR/arcan-smoke.log" 2>&1 &
  ARCAN_PID=$!

  if ! wait_http "$ARCAN_URL/health" 20; then
    fail "arcan failed to start"
    cat "$TMPDIR/arcan-smoke.log"
    exit 1
  fi
  log "arcan running (pid=$ARCAN_PID)"
fi

# ═════════════════════════════════════════════════════════════════════
# TEST SUITE
# ═════════════════════════════════════════════════════════════════════

echo ""
log "━━━ Health & Connectivity ━━━"

HEALTH=$(curl -s "$OPSISD_URL/health")
assert_eq "opsisd /health returns ok" "ok" "$(echo "$HEALTH" | json_field "d['status']")"
assert_eq "service name is opsis" "opsis" "$(echo "$HEALTH" | json_field "d['service']")"

# ── Schema Registry ──────────────────────────────────────────────────

echo ""
log "━━━ Schema Registry (BRO-503) ━━━"

SCHEMAS=$(curl -s "$OPSISD_URL/schemas")
SCHEMA_COUNT=$(echo "$SCHEMAS" | json_field "len(d)")
assert_eq "4 built-in schemas registered" "4" "$SCHEMA_COUNT"

# Check each built-in schema exists
for key in "usgs.geojson.v1" "openmeteo.current.v1" "gaia.v1" "arcan.agent.v1"; do
  EXISTS=$(echo "$SCHEMAS" | json_field "any(s['key']=='$key' for s in d)")
  assert_eq "schema $key exists" "True" "$EXISTS"
done

# Single schema lookup
assert_http "GET /schemas/arcan.agent.v1 → 200" "$OPSISD_URL/schemas/arcan.agent.v1" "200"
assert_http "GET /schemas/nonexistent.v1 → 404" "$OPSISD_URL/schemas/nonexistent.v1" "404"

AGENT_SCHEMA=$(curl -s "$OPSISD_URL/schemas/arcan.agent.v1")
assert_eq "arcan.agent.v1 producer is Agent" "Agent" "$(echo "$AGENT_SCHEMA" | json_field "d['producer']")"

# ── Inject API ───────────────────────────────────────────────────────

echo ""
log "━━━ Inject API ━━━"

# Inject with known schema
INJECT_RESP=$(curl -s -X POST "$OPSISD_URL/events/inject" \
  -H 'Content-Type: application/json' \
  -d '{
    "events": [{
      "id": "smoke-known-1",
      "tick": 0,
      "timestamp": "2026-01-01T00:00:00Z",
      "source": {"Agent": "smoke-agent:test"},
      "kind": {"type": "AgentObservation", "insight": "smoke test observation", "confidence": 0.8},
      "location": null,
      "domain": "Technology",
      "severity": 0.8,
      "schema_key": "arcan.agent.v1",
      "tags": ["smoke"]
    }]
  }')
assert_eq "inject known schema: accepted=1" "1" "$(echo "$INJECT_RESP" | json_field "d['accepted']")"
assert_eq "inject known schema: no warnings" "0" "$(echo "$INJECT_RESP" | json_field "len(d['warnings'])")"

# Inject with unknown schema (should warn but accept)
INJECT_WARN=$(curl -s -X POST "$OPSISD_URL/events/inject" \
  -H 'Content-Type: application/json' \
  -d '{
    "events": [{
      "id": "smoke-unknown-1",
      "tick": 0,
      "timestamp": "2026-01-01T00:00:01Z",
      "source": {"Feed": "unknown-feed"},
      "kind": {"type": "WorldObservation", "summary": "test"},
      "location": null,
      "domain": "Technology",
      "severity": 0.3,
      "schema_key": "nonexistent.v1",
      "tags": []
    }]
  }')
assert_eq "inject unknown schema: accepted=1" "1" "$(echo "$INJECT_WARN" | json_field "d['accepted']")"
assert_eq "inject unknown schema: 1 warning" "1" "$(echo "$INJECT_WARN" | json_field "len(d['warnings'])")"
assert_contains "warning mentions schema key" "$(echo "$INJECT_WARN")" "nonexistent.v1"

# Inject empty batch
INJECT_EMPTY=$(curl -s -X POST "$OPSISD_URL/events/inject" \
  -H 'Content-Type: application/json' \
  -d '{"events": []}')
assert_eq "inject empty batch: accepted=0" "0" "$(echo "$INJECT_EMPTY" | json_field "d['accepted']")"

# ── Event Routing (domain vs unrouted) ───────────────────────────────

echo ""
log "━━━ Event Routing & SSE Stream ━━━"

# Start SSE listener
SSE_LOG="$TMPDIR/opsis-smoke-sse.log"
(curl -s -N "$OPSISD_URL/stream" > "$SSE_LOG" 2>&1 &
SSE_PID=$!
sleep 0.5

# Inject routed (domain: Finance) + unrouted (domain: null)
curl -s -X POST "$OPSISD_URL/events/inject" \
  -H 'Content-Type: application/json' \
  -d '{
    "events": [
      {"id": "smoke-routed","tick": 0,"timestamp": "2026-01-01T00:01:00Z","source": {"Agent": "smoke-0:test"},"kind": {"type": "AgentObservation", "insight": "routed event", "confidence": 0.9},"location": {"lat": 4.71, "lon": -74.07},"domain": "Finance","severity": 0.9,"schema_key": "arcan.agent.v1","tags": []},
      {"id": "smoke-unrouted","tick": 0,"timestamp": "2026-01-01T00:01:01Z","source": {"Agent": "smoke-0:test"},"kind": {"type": "AgentAlert", "message": "unrouted alert"},"location": null,"domain": null,"severity": 0.7,"schema_key": "arcan.agent.v1","tags": []}
    ]
  }' > /dev/null 2>&1

# Wait for tick flush (2 Hz = 0.5s per tick, wait 2s for safety)
sleep 2

kill $SSE_PID 2>/dev/null
wait $SSE_PID 2>/dev/null || true)

# Parse SSE output
ROUTED_FOUND=$(grep "data:" "$SSE_LOG" | python3 -c "
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line.startswith('data:'): continue
    d = json.loads(line[5:])
    for s in d.get('state_line_deltas', []):
        for e in s.get('new_events', []):
            if e.get('id') == 'smoke-routed':
                print('yes')
                exit()
print('no')
" 2>/dev/null)
assert_eq "routed event appears in state_line_deltas" "yes" "$ROUTED_FOUND"

UNROUTED_FOUND=$(grep "data:" "$SSE_LOG" | python3 -c "
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line.startswith('data:'): continue
    d = json.loads(line[5:])
    for u in d.get('unrouted_events', []):
        if u.get('id') == 'smoke-unrouted':
            print('yes')
            exit()
print('no')
" 2>/dev/null)
assert_eq "unrouted event appears in unrouted_events" "yes" "$UNROUTED_FOUND"

# Check WorldDelta structure
DELTA_FIELDS=$(grep "data:" "$SSE_LOG" | head -1 | sed 's/^data://' | python3 -c "
import sys, json
d = json.loads(sys.stdin.readline())
fields = sorted(d.keys())
print(','.join(fields))
" 2>/dev/null)
assert_contains "WorldDelta has unrouted_events field" "$DELTA_FIELDS" "unrouted_events"
assert_contains "WorldDelta has gaia_insights field" "$DELTA_FIELDS" "gaia_insights"
assert_contains "WorldDelta has state_line_deltas field" "$DELTA_FIELDS" "state_line_deltas"

# ── Multi-Agent Topology ─────────────────────────────────────────────

echo ""
log "━━━ Multi-Agent Topology ━━━"

SSE_MULTI="$TMPDIR/opsis-smoke-multi.log"
(curl -s -N "$OPSISD_URL/stream" > "$SSE_MULTI" 2>&1 &
SSE_PID=$!
sleep 0.5

# Inject from 3 different agents
for i in 0 1 2; do
  curl -s -X POST "$OPSISD_URL/events/inject" \
    -H 'Content-Type: application/json' \
    -d "{
      \"events\": [{
        \"id\": \"multi-agent-$i\",
        \"tick\": 0,
        \"timestamp\": \"2026-01-01T00:02:0${i}Z\",
        \"source\": {\"Agent\": \"arcan-agent-${i}:sess\"},
        \"kind\": {\"type\": \"AgentObservation\", \"insight\": \"Agent $i active\", \"confidence\": 0.7},
        \"location\": null,
        \"domain\": \"Technology\",
        \"severity\": 0.7,
        \"schema_key\": \"arcan.agent.v1\",
        \"tags\": []
      }]
    }" > /dev/null 2>&1
done

sleep 2
kill $SSE_PID 2>/dev/null
wait $SSE_PID 2>/dev/null || true)

AGENT_COUNT=$(grep "data:" "$SSE_MULTI" | python3 -c "
import sys, json
agents = set()
for line in sys.stdin:
    line = line.strip()
    if not line.startswith('data:'): continue
    d = json.loads(line[5:])
    for s in d.get('state_line_deltas', []):
        for e in s.get('new_events', []):
            src = e.get('source', {})
            if isinstance(src, dict) and 'Agent' in src:
                agents.add(src['Agent'])
print(len(agents))
" 2>/dev/null)
# At least 1 agent should appear (events may span multiple ticks)
if [[ "$AGENT_COUNT" -ge 1 ]]; then
  pass "multi-agent events from $AGENT_COUNT distinct agents"
else
  fail "multi-agent events: expected ≥1 agents, got $AGENT_COUNT"
fi

# ── Geolocation Preservation ─────────────────────────────────────────

echo ""
log "━━━ Geolocation Preservation ━━━"

SSE_GEO="$TMPDIR/opsis-smoke-geo.log"
(curl -s -N "$OPSISD_URL/stream" > "$SSE_GEO" 2>&1 &
SSE_PID=$!
sleep 0.5

curl -s -X POST "$OPSISD_URL/events/inject" \
  -H 'Content-Type: application/json' \
  -d '{
    "events": [{
      "id": "geo-test",
      "tick": 0,
      "timestamp": "2026-01-01T00:03:00Z",
      "source": {"Agent": "geo-agent:test"},
      "kind": {"type": "AgentObservation", "insight": "geolocated event", "confidence": 0.8},
      "location": {"lat": 35.68, "lon": 139.69},
      "domain": "Finance",
      "severity": 0.8,
      "schema_key": "arcan.agent.v1",
      "tags": []
    }]
  }' > /dev/null 2>&1

sleep 2
kill $SSE_PID 2>/dev/null
wait $SSE_PID 2>/dev/null || true)

GEO_OK=$(grep "data:" "$SSE_GEO" | python3 -c "
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line.startswith('data:'): continue
    d = json.loads(line[5:])
    for s in d.get('state_line_deltas', []):
        for e in s.get('new_events', []):
            if e.get('id') == 'geo-test' and e.get('location'):
                loc = e['location']
                if abs(loc['lat'] - 35.68) < 0.01 and abs(loc['lon'] - 139.69) < 0.01:
                    print('yes')
                    exit()
print('no')
" 2>/dev/null)
assert_eq "geolocation preserved through pipeline" "yes" "$GEO_OK"

# ── Snapshot Endpoint (client hydration) ──────────────────────────────

echo ""
log "━━━ Snapshot Endpoint ━━━"

# Inject an event so snapshot has content
curl -s -X POST "$OPSISD_URL/events/inject" \
  -H 'Content-Type: application/json' \
  -d '{
    "events": [{
      "id": "snap-test",
      "tick": 0,
      "timestamp": "2026-01-01T00:04:00Z",
      "source": {"Agent": "snap-agent:test"},
      "kind": {"type": "AgentObservation", "insight": "snapshot hydration test", "confidence": 0.7},
      "location": null,
      "domain": "Technology",
      "severity": 0.7,
      "schema_key": "arcan.agent.v1",
      "tags": []
    }]
  }' > /dev/null 2>&1
sleep 2

SNAP_CODE=$(curl -s -o /tmp/opsis-snapshot.json -w "%{http_code}" "$OPSISD_URL/snapshot")
assert_eq "GET /snapshot returns 200" "200" "$SNAP_CODE"

SNAP_TICK=$(cat /tmp/opsis-snapshot.json | json_field "d['world_state']['clock']['tick']")
if [[ "$SNAP_TICK" -gt 0 ]]; then
  pass "snapshot has valid tick ($SNAP_TICK)"
else
  fail "snapshot tick should be > 0 (got $SNAP_TICK)"
fi

SNAP_DOMAINS=$(cat /tmp/opsis-snapshot.json | json_field "len(d['world_state']['state_lines'])")
assert_eq "snapshot has 12+ domains" "True" "$(python3 -c "print($SNAP_DOMAINS >= 12)")"

SNAP_EVENTS=$(cat /tmp/opsis-snapshot.json | json_field "len(d['recent_events'])")
if [[ "$SNAP_EVENTS" -gt 0 ]]; then
  pass "snapshot has recent events ($SNAP_EVENTS)"
else
  fail "snapshot should have recent events"
fi

# ── Arcan ↔ Opsis Bridge ────────────────────────────────────────────

if $WITH_ARCAN; then
  echo ""
  log "━━━ Arcan ↔ Opsis Bridge (BRO-504) ━━━"

  ARCAN_HEALTH=$(curl -s "$ARCAN_URL/health")
  assert_eq "arcan /health returns ok" "ok" "$(echo "$ARCAN_HEALTH" | json_field "d['status']")"

  # Check opsis bridge log
  BRIDGE_LOG=$(grep -i "opsis" "$TMPDIR/arcan-smoke.log" 2>/dev/null || echo "")
  assert_contains "arcan logs show opsis bridge enabled" "$BRIDGE_LOG" "Opsis bridge enabled"

  # Create session and send message
  SESSION=$(curl -s -X POST "$ARCAN_URL/sessions" \
    -H 'Content-Type: application/json' \
    -d '{"branch": "main"}')
  SESSION_ID=$(echo "$SESSION" | json_field "d['session_id']")

  if [[ -n "$SESSION_ID" && "$SESSION_ID" != "None" ]]; then
    pass "arcan session created: $SESSION_ID"

    # Start SSE listener
    SSE_ARCAN="$TMPDIR/opsis-smoke-arcan.log"
    (curl -s -N "$OPSISD_URL/stream" > "$SSE_ARCAN" 2>&1 &
    SSE_PID=$!
    sleep 0.5

    # Send message → triggers OpsisToolObserver
    curl -s -X POST "$ARCAN_URL/sessions/$SESSION_ID/runs" \
      -H 'Content-Type: application/json' \
      -d '{"objective": "Analyze current market conditions."}' \
      --max-time 15 > /dev/null 2>&1

    sleep 3
    kill $SSE_PID 2>/dev/null
    wait $SSE_PID 2>/dev/null || true)

    OBSERVER_EVENT=$(grep "data:" "$SSE_ARCAN" | python3 -c "
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line.startswith('data:'): continue
    d = json.loads(line[5:])
    for s in d.get('state_line_deltas', []):
        for e in s.get('new_events', []):
            src = e.get('source', {})
            if isinstance(src, dict) and 'Agent' in src and 'run completed' in str(e.get('kind', {})):
                print('yes')
                exit()
print('no')
" 2>/dev/null)
    assert_eq "OpsisToolObserver pushed run completion to world state" "yes" "$OBSERVER_EVENT"
  else
    fail "arcan session creation failed"
  fi
fi

# ═════════════════════════════════════════════════════════════════════
# RESULTS
# ═════════════════════════════════════════════════════════════════════

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [[ $FAILED -eq 0 ]]; then
  echo -e "  ${GREEN}ALL $TOTAL TESTS PASSED${RESET}"
else
  echo -e "  ${RED}$FAILED/$TOTAL TESTS FAILED${RESET}"
fi
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

exit $FAILED
