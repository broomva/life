#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

if [ -n "${CONTROL_SMOKE_CMD:-}" ]; then
  eval "$CONTROL_SMOKE_CMD"
  exit 0
fi

# Multi-workspace monorepo: aiOS + Arcan + Lago + Spaces
if command -v cargo >/dev/null 2>&1; then
  ran=0
  for ws in aiOS arcan lago spaces; do
    if [ -f "$ws/Cargo.toml" ]; then
      (cd "$ws" && cargo check --quiet)
      ran=1
    fi
  done
  [ "$ran" -eq 1 ] && exit 0
fi

if [ -f Cargo.toml ] && command -v cargo >/dev/null 2>&1; then
  cargo check --quiet
  exit 0
fi

if [ -f package.json ] && command -v npm >/dev/null 2>&1; then
  npm run -s build || npm run -s smoke
  exit 0
fi

if [ -f pyproject.toml ] && command -v pytest >/dev/null 2>&1; then
  pytest -q -k smoke || pytest -q -k "not integration and not e2e"
  exit 0
fi

echo "No smoke command detected. Set CONTROL_SMOKE_CMD."
exit 1
