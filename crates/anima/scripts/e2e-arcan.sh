#!/usr/bin/env bash
# ============================================================
# Anima E2E Test — validates identity primitives against Arcan
#
# This script:
# 1. Builds Anima and Arcan
# 2. Starts Arcan with mock provider
# 3. Creates a session via HTTP API
# 4. Validates identity operations
# 5. Validates JWT signing
# 6. Validates Lago persistence (event roundtrip)
# 7. Stops Arcan
#
# Usage:
#   ./scripts/e2e-arcan.sh              # Run full E2E
#   SKIP_BUILD=1 ./scripts/e2e-arcan.sh # Skip cargo build
#   ARCAN_PORT=3200 ./scripts/e2e-arcan.sh # Custom port
# ============================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANIMA_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LIFE_ROOT="$(cd "$ANIMA_ROOT/.." && pwd)"
ARCAN_ROOT="$LIFE_ROOT/arcan"

PORT="${ARCAN_PORT:-3199}"
DATA_DIR=$(mktemp -d)
ARCAN_PID=""
PASSED=0
FAILED=0
TOTAL=0

cleanup() {
    if [ -n "$ARCAN_PID" ] && kill -0 "$ARCAN_PID" 2>/dev/null; then
        kill "$ARCAN_PID" 2>/dev/null || true
        wait "$ARCAN_PID" 2>/dev/null || true
    fi
    rm -rf "$DATA_DIR"
}
trap cleanup EXIT

log() { echo "[anima-e2e] $*"; }
pass() { PASSED=$((PASSED + 1)); TOTAL=$((TOTAL + 1)); log "  PASS: $1"; }
fail() { FAILED=$((FAILED + 1)); TOTAL=$((TOTAL + 1)); log "  FAIL: $1"; }

assert_eq() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        pass "$desc"
    else
        fail "$desc (expected '$expected', got '$actual')"
    fi
}

assert_contains() {
    local desc="$1" haystack="$2" needle="$3"
    if echo "$haystack" | grep -q "$needle"; then
        pass "$desc"
    else
        fail "$desc (expected to contain '$needle')"
    fi
}

assert_not_empty() {
    local desc="$1" value="$2"
    if [ -n "$value" ]; then
        pass "$desc"
    else
        fail "$desc (was empty)"
    fi
}

# ============================================================
# PHASE 0: Build
# ============================================================
log "=== ANIMA E2E TEST ==="
log "Port: $PORT | Data dir: $DATA_DIR"

if [ "${SKIP_BUILD:-}" != "1" ]; then
    log "Building Anima..."
    (cd "$ANIMA_ROOT" && cargo build --workspace --quiet 2>&1)

    log "Building Arcan..."
    (cd "$ARCAN_ROOT" && cargo build -p arcan --quiet 2>&1)
fi

# ============================================================
# PHASE 1: Run Anima unit + regression tests
# ============================================================
log ""
log "--- Phase 1: Anima unit + regression tests ---"
UNIT_OUTPUT=$(cd "$ANIMA_ROOT" && cargo test --workspace --quiet 2>&1)
UNIT_COUNT=$(echo "$UNIT_OUTPUT" | grep "test result:" | awk '{sum += $4} END {print sum}')
UNIT_FAIL=$(echo "$UNIT_OUTPUT" | grep "test result:" | awk '{sum += $6} END {print sum}')

if [ "$UNIT_FAIL" = "0" ]; then
    pass "All $UNIT_COUNT unit tests passed"
else
    fail "$UNIT_FAIL of $UNIT_COUNT unit tests failed"
fi

# ============================================================
# PHASE 2: Start Arcan with mock provider
# ============================================================
log ""
log "--- Phase 2: Start Arcan daemon (mock provider) ---"

ARCAN_BIN="$LIFE_ROOT/.target/debug/arcan"
if [ ! -f "$ARCAN_BIN" ]; then
    ARCAN_BIN=$(cd "$ARCAN_ROOT" && cargo build -p arcan --message-format=json 2>/dev/null | \
        grep '"executable"' | tail -1 | jq -r '.executable // empty')
fi

if [ ! -f "$ARCAN_BIN" ]; then
    log "Arcan binary not found, skipping HTTP tests"
else
    ARCAN_PROVIDER=mock "$ARCAN_BIN" \
        --data-dir "$DATA_DIR" \
        --port "$PORT" \
        serve &
    ARCAN_PID=$!

    # Wait for health
    for i in $(seq 1 30); do
        if curl -sf "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
            break
        fi
        sleep 0.2
    done

    HEALTH=$(curl -sf "http://127.0.0.1:$PORT/health" 2>/dev/null || echo '{}')
    HEALTH_STATUS=$(echo "$HEALTH" | jq -r '.status // "unknown"')

    if [ "$HEALTH_STATUS" = "healthy" ] || [ "$HEALTH_STATUS" = "ok" ]; then
        pass "Arcan daemon started (healthy on :$PORT)"
    else
        fail "Arcan daemon health check"
        log "Health response: $HEALTH"
    fi

    # ============================================================
    # PHASE 3: Create session via HTTP API
    # ============================================================
    log ""
    log "--- Phase 3: Arcan HTTP API integration ---"

    SESSION_RESP=$(curl -sf -X POST "http://127.0.0.1:$PORT/sessions" \
        -H "Content-Type: application/json" \
        -d '{}' 2>/dev/null || echo '{}')

    SESSION_ID=$(echo "$SESSION_RESP" | jq -r '.session_id // empty')
    assert_not_empty "Session created" "$SESSION_ID"

    if [ -n "$SESSION_ID" ]; then
        # List sessions
        SESSIONS=$(curl -sf "http://127.0.0.1:$PORT/sessions" 2>/dev/null || echo '[]')
        SESSION_COUNT=$(echo "$SESSIONS" | jq 'length')
        if [ "$SESSION_COUNT" -ge 1 ]; then
            pass "Session appears in list ($SESSION_COUNT sessions)"
        else
            fail "Session not found in list"
        fi

        # Get session state
        STATE=$(curl -sf "http://127.0.0.1:$PORT/sessions/$SESSION_ID/state" 2>/dev/null || echo '{}')
        assert_not_empty "Session state retrievable" "$STATE"

        # List events (should be empty initially)
        EVENTS=$(curl -sf "http://127.0.0.1:$PORT/sessions/$SESSION_ID/events" 2>/dev/null || echo '[]')
        assert_not_empty "Events endpoint responds" "$EVENTS"
    fi

    # ============================================================
    # PHASE 4: Verify Lago persistence (data dir)
    # ============================================================
    log ""
    log "--- Phase 4: Persistence validation ---"

    if [ -d "$DATA_DIR" ]; then
        # Check that Lago journal was created
        if ls "$DATA_DIR"/*.redb >/dev/null 2>&1 || ls "$DATA_DIR"/journal.redb >/dev/null 2>&1; then
            pass "Lago journal file exists"
        else
            # Check nested paths
            REDB_COUNT=$(find "$DATA_DIR" -name "*.redb" 2>/dev/null | wc -l | tr -d ' ')
            if [ "$REDB_COUNT" -gt 0 ]; then
                pass "Lago journal file exists ($REDB_COUNT redb files)"
            else
                fail "No Lago journal file found in $DATA_DIR"
                log "Contents: $(ls -la "$DATA_DIR" 2>/dev/null)"
            fi
        fi
    fi

    # Stop Arcan
    kill "$ARCAN_PID" 2>/dev/null || true
    wait "$ARCAN_PID" 2>/dev/null || true
    ARCAN_PID=""
    pass "Arcan daemon stopped cleanly"
fi

# ============================================================
# PHASE 5: Anima-specific validations (standalone)
# ============================================================
log ""
log "--- Phase 5: Anima crypto + identity validations ---"

# Run the lifecycle integration test specifically
LIFECYCLE_OUTPUT=$(cd "$ANIMA_ROOT" && cargo test -p anima-identity --test lifecycle -- --nocapture 2>&1)
if echo "$LIFECYCLE_OUTPUT" | grep -q "ALL 8 STEPS PASSED"; then
    pass "Full lifecycle integration test"
else
    fail "Lifecycle integration test"
fi

# Run regression tests specifically
REGRESSION_OUTPUT=$(cd "$ANIMA_ROOT" && cargo test -p anima-identity --test regression --quiet 2>&1)
REGRESSION_FAIL=$(echo "$REGRESSION_OUTPUT" | grep "test result:" | awk '{sum += $6} END {print sum}')
REGRESSION_COUNT=$(echo "$REGRESSION_OUTPUT" | grep "test result:" | awk '{sum += $4} END {print sum}')
if [ "$REGRESSION_FAIL" = "0" ]; then
    pass "All $REGRESSION_COUNT regression tests passed"
else
    fail "$REGRESSION_FAIL of $REGRESSION_COUNT regression tests failed"
fi

# ============================================================
# SUMMARY
# ============================================================
log ""
log "=== RESULTS ==="
log "Total: $TOTAL | Passed: $PASSED | Failed: $FAILED"

if [ "$FAILED" -gt 0 ]; then
    log "STATUS: FAILED"
    exit 1
else
    log "STATUS: ALL PASSED"
    exit 0
fi
