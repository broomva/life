#!/usr/bin/env bash
set -euo pipefail

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"

echo "=== Test: SpacetimeDB module build ==="
cd "$root_dir/spacetimedb"
cargo check --quiet --target wasm32-unknown-unknown 2>/dev/null || cargo check --quiet

echo "=== Test: CLI client build ==="
cd "$root_dir"
cargo build --quiet

echo "Tests OK"
