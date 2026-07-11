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

# Single root virtual workspace (crates/<cluster>/<crate>) — BRO-1858.
# `--workspace` at the root mirrors .github/workflows/ci.yml's Test (Linux) gate.
# The old per-subdir loop was stale dead code (those dirs no longer carry Cargo.toml).
if [ -f Cargo.toml ] && command -v cargo >/dev/null 2>&1; then
  cargo test --workspace --quiet
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
