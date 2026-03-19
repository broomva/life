#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

if [ -n "${CONTROL_CHECK_CMD:-}" ]; then
  eval "$CONTROL_CHECK_CMD"
  exit 0
fi

if [ -f Cargo.toml ] && command -v cargo >/dev/null 2>&1; then
  cargo clippy --all-targets --all-features -- -D warnings
  exit 0
fi

if [ -f package.json ] && command -v npm >/dev/null 2>&1; then
  npm run -s lint
  npm run -s typecheck || true
  exit 0
fi

if [ -f pyproject.toml ]; then
  if command -v ruff >/dev/null 2>&1; then
    ruff check .
  fi
  if command -v mypy >/dev/null 2>&1; then
    mypy .
  fi
  exit 0
fi

echo "No check command detected. Set CONTROL_CHECK_CMD."
exit 1
