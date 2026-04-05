#!/usr/bin/env bash
set -euo pipefail

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"

echo "=== Smoke: SpacetimeDB module ==="
cd "$root_dir/spacetimedb"
cargo check --quiet --target wasm32-unknown-unknown 2>/dev/null || cargo check --quiet

echo "=== Smoke: CLI client ==="
cd "$root_dir"
cargo check --quiet

echo "Smoke OK"
