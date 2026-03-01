#!/usr/bin/env bash
set -euo pipefail

# CLI smoke test — exercises built binaries
# Can be run standalone with APP_CLI_BIN or as part of scripts/control/cli_e2e.sh

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)

passed=0
failed=0

ok()   { echo "[PASS] $1"; passed=$((passed + 1)); }
fail() { echo "[FAIL] $1"; failed=$((failed + 1)); }

# If APP_CLI_BIN is set, test that specific binary
if [ -n "${APP_CLI_BIN:-}" ]; then
  if "$APP_CLI_BIN" --help >/dev/null 2>&1; then
    ok "$APP_CLI_BIN --help"
  else
    fail "$APP_CLI_BIN --help"
  fi

  if [ -n "${APP_CLI_VERSION_ARG:-}" ]; then
    if "$APP_CLI_BIN" "$APP_CLI_VERSION_ARG" >/dev/null 2>&1; then
      ok "$APP_CLI_BIN $APP_CLI_VERSION_ARG"
    else
      fail "$APP_CLI_BIN $APP_CLI_VERSION_ARG"
    fi
  fi
else
  # Default: test all known binaries if they exist
  for bin in "$root/lago/target/debug/lago-cli" "$root/lago/target/debug/lagod" "$root/arcan/target/debug/arcan"; do
    if [ -x "$bin" ]; then
      name=$(basename "$bin")
      if "$bin" --help >/dev/null 2>&1; then
        ok "$name --help"
      else
        fail "$name --help"
      fi
    fi
  done
fi

total=$((passed + failed))
if [ "$total" -eq 0 ]; then
  echo "No CLI binaries found. Build first or set APP_CLI_BIN." >&2
  exit 1
fi

echo "CLI smoke: $passed/$total passed"
[ "$failed" -eq 0 ]
