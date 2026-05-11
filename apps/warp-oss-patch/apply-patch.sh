#!/usr/bin/env bash
# Clone warpdotdev/warp, apply our one-line OSS-channel override patch,
# and build the binary. Re-running is idempotent: the upstream clone is
# refreshed and the patch reapplied.
#
# Pre-flight finding 2026-05-11: the OSS build silently drops endpoint
# override env vars via a release-channel gate. This patch removes the
# gate for Channel::Oss only.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UPSTREAM_DIR="$SCRIPT_DIR/upstream"
PATCH_FILE="$SCRIPT_DIR/channel-oss-override.patch"
UPSTREAM_URL="https://github.com/warpdotdev/warp.git"

# Clone or refresh upstream.
if [ -d "$UPSTREAM_DIR/.git" ]; then
  echo "==> Refreshing upstream clone"
  DEFAULT_BRANCH=$(git -C "$UPSTREAM_DIR" symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null | sed 's|^origin/||' || echo "master")
  git -C "$UPSTREAM_DIR" fetch --depth 1 origin "$DEFAULT_BRANCH"
  git -C "$UPSTREAM_DIR" reset --hard FETCH_HEAD
  # Drop any prior patch state before reapplying.
  git -C "$UPSTREAM_DIR" checkout -- .
else
  echo "==> Cloning upstream"
  git clone --depth 1 "$UPSTREAM_URL" "$UPSTREAM_DIR"
fi

# Apply patch. Skip if already applied (idempotent).
echo "==> Applying channel-oss-override.patch"
if git -C "$UPSTREAM_DIR" apply --reverse --check "$PATCH_FILE" 2>/dev/null; then
  echo "==> Patch already applied; skipping"
else
  git -C "$UPSTREAM_DIR" apply --check "$PATCH_FILE"
  git -C "$UPSTREAM_DIR" apply "$PATCH_FILE"
fi

# Build the OSS binary in release mode.
echo "==> Building warp-oss (release)"
( cd "$UPSTREAM_DIR" && cargo build --release --bin warp-oss )

BIN="$UPSTREAM_DIR/target/release/warp-oss"
if [ -x "$BIN" ]; then
  echo "==> Built: $BIN"
else
  echo "ERROR: expected binary not found at $BIN" >&2
  exit 1
fi
