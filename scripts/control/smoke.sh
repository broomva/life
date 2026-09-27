#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$root"

if [ -n "${CONTROL_SMOKE_CMD:-}" ]; then
  eval "$CONTROL_SMOKE_CMD"
  exit 0
fi

# Single root virtual workspace (crates/<cluster>/<crate>) — BRO-1858.
# The old per-subdir loop (aiOS/arcan/lago/…) is stale: those top-level dirs no
# longer carry Cargo.toml, so `--workspace` at the root is the current model and
# mirrors .github/workflows/ci.yml (aiOS is now a separate repo, not a member).
if [ -f Cargo.toml ] && command -v cargo >/dev/null 2>&1; then
  cargo check --workspace --quiet
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
