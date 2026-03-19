#!/usr/bin/env bash
set -euo pipefail

cli_bin="${APP_CLI_BIN:-}"
if [ -z "$cli_bin" ]; then
  echo "Set APP_CLI_BIN for CLI E2E smoke." >&2
  exit 1
fi

"$cli_bin" --help >/dev/null
if [ -n "${APP_CLI_VERSION_ARG:-}" ]; then
  "$cli_bin" "$APP_CLI_VERSION_ARG" >/dev/null
fi

echo "CLI smoke test passed for $cli_bin"
