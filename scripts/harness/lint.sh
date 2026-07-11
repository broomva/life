#!/usr/bin/env bash
set -euo pipefail

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)

if [ -n "${HARNESS_LINT_CMD:-}" ]; then
  cd "$root_dir"
  eval "$HARNESS_LINT_CMD"
  exit 0
fi

# Single root virtual workspace (crates/<cluster>/<crate>) — BRO-1858.
# Mirror .github/workflows/ci.yml's Lint gate EXACTLY (incl. the
# `-A too_many_arguments` allow) so `make lint` never false-blocks on a lint CI
# permits. The old per-subdir loop was stale (those dirs no longer carry
# Cargo.toml); the old `--all-targets --all-features` fallthrough was stricter
# than ci.yml.
if [ -f "$root_dir/Cargo.toml" ] && command -v cargo >/dev/null 2>&1; then
  cd "$root_dir"
  cargo clippy --workspace -- -D warnings -A clippy::too_many_arguments
  exit 0
fi

if [ -f "$root_dir/package.json" ] && command -v node >/dev/null 2>&1 && command -v npm >/dev/null 2>&1; then
  cd "$root_dir"
  if node -e 'const p=require("./package.json"); process.exit(p.scripts&&p.scripts.lint?0:1)' >/dev/null 2>&1; then
    npm run -s lint
    exit 0
  fi
fi

if [ -f "$root_dir/pyproject.toml" ]; then
  cd "$root_dir"
  if command -v ruff >/dev/null 2>&1; then
    ruff check .
    exit 0
  fi
  if command -v flake8 >/dev/null 2>&1; then
    flake8 .
    exit 0
  fi
fi

echo "No default lint command detected."
echo "Set HARNESS_LINT_CMD or customize scripts/harness/lint.sh."
exit 1
