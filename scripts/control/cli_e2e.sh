#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

if [ -n "${CONTROL_CLI_E2E_CMD:-}" ]; then
  eval "$CONTROL_CLI_E2E_CMD"
  exit 0
fi

if [ -x ./tests/e2e/cli/smoke.sh ]; then
  ./tests/e2e/cli/smoke.sh
  exit 0
fi

cli_bin="${APP_CLI_BIN:-}"
if [ -n "$cli_bin" ]; then
  "$cli_bin" --help >/dev/null
  echo "CLI reachable: $cli_bin"
  exit 0
fi

echo "No CLI e2e command configured. Set CONTROL_CLI_E2E_CMD or APP_CLI_BIN." >&2
exit 1
