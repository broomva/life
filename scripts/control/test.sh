#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

# Environment-first: auto-load rustup cargo path when available.
if ! command -v cargo >/dev/null 2>&1 && [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1090
  . "$HOME/.cargo/env"
fi

if [ -n "${CONTROL_TEST_CMD:-}" ]; then
  eval "$CONTROL_TEST_CMD"
  exit 0
fi

# Multi-workspace monorepo: aiOS + Arcan + Lago + Autonomic + Spaces
if command -v cargo >/dev/null 2>&1; then
  ran=0
  for ws in aiOS arcan lago autonomic praxis vigil spaces; do
    if [ -f "$ws/Cargo.toml" ]; then
      (cd "$ws" && cargo test --workspace --quiet)
      ran=1
    fi
  done
  [ "$ran" -eq 1 ] && exit 0
fi

if [ -f Cargo.toml ] && command -v cargo >/dev/null 2>&1; then
  cargo test --quiet
  exit 0
fi

if [ -f package.json ] && command -v npm >/dev/null 2>&1; then
  npm run -s test
  exit 0
fi

if [ -f pyproject.toml ] && command -v pytest >/dev/null 2>&1; then
  pytest -q
  exit 0
fi

echo "No test command detected. Set CONTROL_TEST_CMD."
exit 1
