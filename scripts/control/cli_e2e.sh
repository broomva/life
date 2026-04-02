#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

# Allow override for custom environments
if [ -n "${CONTROL_CLI_E2E_CMD:-}" ]; then
  eval "$CONTROL_CLI_E2E_CMD"
  exit 0
fi

# Allow delegating to external test script
if [ -x ./tests/e2e/cli/smoke.sh ] && [ -n "${APP_CLI_BIN:-}" ]; then
  ./tests/e2e/cli/smoke.sh
  exit 0
fi

passed=0
failed=0

ok()   { echo "[PASS] $1"; passed=$((passed + 1)); }
fail() { echo "[FAIL] $1"; failed=$((failed + 1)); }

echo "CLI E2E: Building and exercising CLI binaries"
echo

# --- Lago CLI ---
if [ -f lago/Cargo.toml ]; then
  echo "--- lago-cli ---"

  # Build (package is named `lago` in lago/crates/lago-cli)
  if (cd lago && cargo build -p lago --quiet); then
    ok "lago-cli builds"
  else
    fail "lago-cli build"
  fi

  lago_bin=""
  for candidate in "lago/.target/debug/lago-cli" "lago/.target/debug/lago" "lago/target/debug/lago" ".target/debug/lago" "target/debug/lago"; do
    if [ -x "$candidate" ]; then
      lago_bin="$candidate"
      break
    fi
  done
  if [ -n "$lago_bin" ]; then
    # --help flag
    if "$lago_bin" --help >/dev/null 2>&1; then
      ok "lago-cli --help"
    else
      fail "lago-cli --help"
    fi

    # init command (in temp dir)
    tmpdir=$(mktemp -d)
    if "$lago_bin" init "$tmpdir/test-repo" >/dev/null 2>&1; then
      ok "lago-cli init"
      # Verify init created expected files
      if [ -f "$tmpdir/test-repo/lago.toml" ]; then
        ok "lago-cli init creates lago.toml"
      else
        fail "lago-cli init creates lago.toml"
      fi
    else
      fail "lago-cli init"
    fi
    rm -rf "$tmpdir"
  fi
  echo
fi

# --- lagod ---
if [ -f lago/Cargo.toml ]; then
  echo "--- lagod ---"

  if (cd lago && cargo build -p lagod --quiet 2>/dev/null); then
    ok "lagod builds"
  else
    fail "lagod build"
  fi

  lagod_bin=""
  for candidate in "lago/.target/debug/lagod" "lago/target/debug/lagod" ".target/debug/lagod" "target/debug/lagod"; do
    if [ -x "$candidate" ]; then
      lagod_bin="$candidate"
      break
    fi
  done
  if [ -n "$lagod_bin" ]; then
    if "$lagod_bin" --help >/dev/null 2>&1; then
      ok "lagod --help"
    else
      fail "lagod --help"
    fi
  fi
  echo
fi

# --- Arcan ---
if [ -f arcan/Cargo.toml ]; then
  echo "--- arcan ---"

  if (cd arcan && cargo build -p arcan --quiet 2>/dev/null); then
    ok "arcan binary builds"
  else
    fail "arcan binary build"
  fi

  arcan_bin=""
  for candidate in "arcan/.target/debug/arcan" "arcan/target/debug/arcan" ".target/debug/arcan" "target/debug/arcan"; do
    if [ -x "$candidate" ]; then
      arcan_bin="$candidate"
      break
    fi
  done
  if [ -n "$arcan_bin" ]; then
    if "$arcan_bin" --help >/dev/null 2>&1; then
      ok "arcan --help"
    else
      fail "arcan --help"
    fi

    # --- Arcan Shell E2E (Levels 1-3, mock provider, no API key) ---
    data_dir=$(mktemp -d)

    # Level 1: Shell boot + slash commands
    shell_out=$(printf '/help\n/status\n/context\n/memory\n/consolidate\n' \
      | "$arcan_bin" shell --provider mock --data-dir "$data_dir" --budget 10.0 -y 2>&1 || true)

    if echo "$shell_out" | grep -q "Tools: 17"; then ok "arcan shell boots (17 tools)"; else fail "arcan shell boots"; fi
    if echo "$shell_out" | grep -q "evaluators active"; then ok "arcan shell nous evaluators"; else fail "arcan shell nous evaluators"; fi
    if echo "$shell_out" | grep -q "Available commands:"; then ok "arcan shell /help"; else fail "arcan shell /help"; fi
    if echo "$shell_out" | grep -q "CACHEABLE"; then ok "arcan shell /context"; else fail "arcan shell /context"; fi
    if echo "$shell_out" | grep -q "consolidation"; then ok "arcan shell /consolidate"; else fail "arcan shell /consolidate"; fi
    if [ -f "$data_dir/shell-journals/"*.redb ] 2>/dev/null; then ok "arcan shell redb journal"; else fail "arcan shell redb journal"; fi
    if [ -d "$data_dir/workspace.lance" ]; then ok "arcan shell lance workspace"; else fail "arcan shell lance workspace"; fi
    if [ -f "$data_dir/memory/MEMORY.md" ]; then ok "arcan shell MEMORY.md"; else fail "arcan shell MEMORY.md"; fi

    # Level 2: Tool execution + Nous safety
    data_dir2=$(mktemp -d)
    tool_out=$(printf 'file\n/status\n' \
      | "$arcan_bin" shell --provider mock --data-dir "$data_dir2" -y 2>&1 || true)

    if echo "$tool_out" | grep -q "\[tool: write_file\]"; then ok "arcan shell tool execution"; else fail "arcan shell tool execution"; fi
    if echo "$tool_out" | grep -q "safety_compliance"; then ok "arcan shell nous safety score"; else fail "arcan shell nous safety score"; fi

    # Level 3: Session resume
    data_dir3=$(mktemp -d)
    printf 'ping\n' | "$arcan_bin" shell --provider mock --data-dir "$data_dir3" -y >/dev/null 2>&1 || true
    sess_id=$(ls "$data_dir3/shell-journals/" 2>/dev/null | head -1 | sed 's/\.redb$//')
    if [ -n "$sess_id" ]; then
      resume_out=$(printf '/history\n' \
        | "$arcan_bin" shell --provider mock --data-dir "$data_dir3" --session "$sess_id" --resume -y 2>&1 || true)
      if echo "$resume_out" | grep -q "Restored.*messages"; then ok "arcan shell session resume"; else fail "arcan shell session resume"; fi
    else
      fail "arcan shell session resume (no journal)"
    fi

    rm -rf "$data_dir" "$data_dir2" "$data_dir3"
  fi
  echo
fi

# --- Nous (nousd) ---
if [ -f nous/Cargo.toml ]; then
  echo "--- nousd ---"

  if (cd nous && cargo build -p nousd --quiet 2>/dev/null); then
    ok "nousd builds"
  else
    fail "nousd build"
  fi

  nousd_bin=""
  for candidate in "nous/.target/debug/nousd" "nous/target/debug/nousd" ".target/debug/nousd" "target/debug/nousd"; do
    if [ -x "$candidate" ]; then
      nousd_bin="$candidate"
      break
    fi
  done
  if [ -n "$nousd_bin" ]; then
    if "$nousd_bin" --help >/dev/null 2>&1; then
      ok "nousd --help"
    else
      fail "nousd --help"
    fi
  fi
  echo
fi

# --- Nous regression (golden fixtures) ---
if [ -f nous/Cargo.toml ]; then
  echo "--- nous golden fixtures ---"

  if (cd nous && cargo test -p nous-heuristics --test golden_replay --quiet 2>/dev/null); then
    ok "nous golden fixture tests"
  else
    fail "nous golden fixture tests"
  fi

  if (cd nous && cargo test -p nous-middleware --test e2e_orchestrator --quiet 2>/dev/null); then
    ok "nous e2e orchestrator tests"
  else
    fail "nous e2e orchestrator tests"
  fi
  echo
fi

# --- Autonomicd ---
if [ -f autonomic/Cargo.toml ]; then
  echo "--- autonomicd ---"

  if (cd autonomic && cargo build -p autonomicd --quiet 2>/dev/null); then
    ok "autonomicd builds"
  else
    fail "autonomicd build"
  fi

  autonomicd_bin=""
  for candidate in "autonomic/.target/debug/autonomicd" "autonomic/target/debug/autonomicd" ".target/debug/autonomicd" "target/debug/autonomicd"; do
    if [ -x "$candidate" ]; then
      autonomicd_bin="$candidate"
      break
    fi
  done
  if [ -n "$autonomicd_bin" ]; then
    if "$autonomicd_bin" --help >/dev/null 2>&1; then
      ok "autonomicd --help"
    else
      fail "autonomicd --help"
    fi
  fi
  echo
fi

# --- Summary ---
total=$((passed + failed))
echo "CLI E2E: $passed/$total passed"

if [ "$failed" -gt 0 ]; then
  echo "CLI E2E: $failed test(s) failed."
  exit 1
fi

echo "CLI E2E: All tests passed."
