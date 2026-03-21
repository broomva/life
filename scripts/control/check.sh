#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

# Environment-first: auto-load rustup cargo path when available.
if ! command -v cargo >/dev/null 2>&1 && [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1090
  . "$HOME/.cargo/env"
fi

if [ -n "${CONTROL_CHECK_CMD:-}" ]; then
  eval "$CONTROL_CHECK_CMD"
  exit 0
fi

# Multi-workspace monorepo: all Life crates (format + lint)
if command -v cargo >/dev/null 2>&1; then
  ran=0
  for ws in aiOS arcan lago autonomic praxis vigil spaces anima haima; do
    if [ -f "$ws/Cargo.toml" ]; then
      (cd "$ws" && cargo fmt --check && cargo clippy --workspace -- -D warnings)
      ran=1
    fi
  done
  [ "$ran" -eq 1 ] && exit 0
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
