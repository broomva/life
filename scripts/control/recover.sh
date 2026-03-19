#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[recover]${NC} $1"; }
warn()  { echo -e "${YELLOW}[recover]${NC} $1"; }
fail()  { echo -e "${RED}[recover]${NC} $1"; }

recovered=0
failures=0
actions_taken=()

# Phase 1: Diagnose which gates are failing
info "Phase 1: Diagnosing gate failures..."
echo

smoke_ok=true
check_ok=true
test_ok=true

for ws in aiOS arcan lago; do
  if [ -f "$ws/Cargo.toml" ]; then
    if ! (cd "$ws" && cargo check --quiet 2>/dev/null); then
      fail "  smoke FAIL: $ws (cargo check failed)"
      smoke_ok=false
    fi
  fi
done

if $smoke_ok; then
  info "  smoke: PASS (all workspaces compile)"
else
  fail "  smoke: FAIL"
fi

# Only check format/lint if smoke passes
if $smoke_ok; then
  for ws in aiOS arcan lago; do
    if [ -f "$ws/Cargo.toml" ]; then
      if ! (cd "$ws" && cargo fmt --check --quiet 2>/dev/null); then
        warn "  check FAIL: $ws (format drift detected)"
        check_ok=false
      fi
      if ! (cd "$ws" && cargo clippy --workspace --quiet -- -D warnings 2>/dev/null); then
        warn "  check FAIL: $ws (clippy warnings)"
        check_ok=false
      fi
    fi
  done

  if $check_ok; then
    info "  check: PASS"
  else
    fail "  check: FAIL"
  fi
fi

# Only run tests if check passes
if $smoke_ok && $check_ok; then
  for ws in aiOS arcan lago; do
    if [ -f "$ws/Cargo.toml" ]; then
      if ! (cd "$ws" && cargo test --workspace --quiet 2>/dev/null); then
        fail "  test FAIL: $ws"
        test_ok=false
      fi
    fi
  done

  if $test_ok; then
    info "  test: PASS"
  else
    fail "  test: FAIL"
  fi
fi

echo

# Phase 2: Attempt automatic recovery for safe fixes
info "Phase 2: Attempting automatic recovery..."
echo

# Recovery action: auto-format (safe, reversible)
if ! $check_ok; then
  for ws in aiOS arcan lago; do
    if [ -f "$ws/Cargo.toml" ]; then
      if ! (cd "$ws" && cargo fmt --check --quiet 2>/dev/null); then
        info "  Applying cargo fmt to $ws..."
        (cd "$ws" && cargo fmt)
        actions_taken+=("cargo fmt ($ws)")
        recovered=$((recovered + 1))
      fi
    fi
  done
fi

echo

# Phase 3: Re-validate after recovery actions
info "Phase 3: Re-validating after recovery..."
echo

final_failures=0

for ws in aiOS arcan lago; do
  if [ -f "$ws/Cargo.toml" ]; then
    if ! (cd "$ws" && cargo check --quiet 2>/dev/null); then
      fail "  $ws: still fails cargo check"
      final_failures=$((final_failures + 1))
    elif ! (cd "$ws" && cargo fmt --check --quiet 2>/dev/null); then
      fail "  $ws: still has format issues"
      final_failures=$((final_failures + 1))
    elif ! (cd "$ws" && cargo clippy --workspace --quiet -- -D warnings 2>/dev/null); then
      warn "  $ws: clippy warnings remain (manual fix required)"
      final_failures=$((final_failures + 1))
    else
      info "  $ws: PASS"
    fi
  fi
done

echo

# Phase 4: Report
info "Recovery summary:"
echo "  Actions taken: ${#actions_taken[@]}"
for action in "${actions_taken[@]+"${actions_taken[@]}"}"; do
  echo "    - $action"
done
echo "  Recovered: $recovered"
echo "  Remaining failures: $final_failures"

if [ "$final_failures" -gt 0 ]; then
  echo
  fail "ESCALATION: $final_failures issue(s) require manual intervention."
  fail "Run 'cargo clippy --workspace' in each failing workspace to see details."
  exit 1
fi

info "All gates recovered successfully."
