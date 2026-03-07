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

  # Build
  if (cd lago && cargo build -p lago-cli --quiet 2>/dev/null); then
    ok "lago-cli builds"
  else
    fail "lago-cli build"
  fi

  lago_bin=".target/debug/lago-cli"
  if [ -x "$lago_bin" ]; then
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

  lagod_bin=".target/debug/lagod"
  if [ -x "$lagod_bin" ]; then
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

  arcan_bin=".target/debug/arcan"
  if [ -x "$arcan_bin" ]; then
    if "$arcan_bin" --help >/dev/null 2>&1; then
      ok "arcan --help"
    else
      fail "arcan --help"
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
