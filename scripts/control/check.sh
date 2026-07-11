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

# Single root virtual workspace (crates/<cluster>/<crate>) — BRO-1858.
# Mirror .github/workflows/ci.yml's Format + Lint gates exactly so `make check`
# (and the pre-push hook) validate the SAME surface as CI. The old per-subdir loop
# was stale dead code (those dirs no longer carry Cargo.toml).
if [ -f Cargo.toml ] && command -v cargo >/dev/null 2>&1; then
  cargo fmt --all -- --check
  cargo clippy --workspace -- -D warnings -A clippy::too_many_arguments
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
