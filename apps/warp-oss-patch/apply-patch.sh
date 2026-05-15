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
PATCH_FILES=(
  "$SCRIPT_DIR/channel-oss-override.patch"
  # Lumen α: makes Metal-shader compilation conditional on the absence of
  # `experimental-wgpu-renderer`. Default build is byte-identical to upstream;
  # the patch only takes effect when the wgpu feature is selected. Required
  # so agents on machines without a working Metal Toolchain install can still
  # produce a runnable binary (via `--features warpui/experimental-wgpu-renderer`).
  "$SCRIPT_DIR/skip-metal-when-wgpu.patch"
  # Lumen α: skip the post-customize browser-sign-in slide when running on
  # Channel::Oss. Without this, onboarding dead-ends at "Sign in on your
  # browser to continue" because warp-oss expects to delegate auth to
  # `app.warp.dev`'s Firebase OAuth flow — which our local lifegw can't
  # speak. Empirically discovered during the 2026-05-15 agent-driven GUI
  # run (see `docs/lumen/validation/2026-05-15/README.md`). The change is
  # a single AND-clause on `requires_login`; all other channels behave
  # identically to upstream.
  "$SCRIPT_DIR/skip-oss-login.patch"
  # Lumen α: rename the OSS build's AppId from ("dev", "warp", "WarpOss")
  # to ("dev", "broomva", "Lumen") so the SQLite / keychain / secure-
  # storage subdir under the team-level Group Container is fully
  # disjoint from stable / preview / dev Warp builds. Without this, an
  # already-running Warp.app holds locks on the shared
  # `2BBY89MBSN.dev.warp/Library/Application Support/dev.warp.WarpOss/`
  # subpath and our binary deadlocks at init (silent log after
  # "Unable to read user from secure storage"). Discovered when the
  # rebuild-then-relaunch flow stopped advancing past that line while
  # `/Applications/Warp.app/Contents/MacOS/stable` was still running.
  # Also retitles the embedded Info.plist (bundle ID, display/CF name,
  # URL scheme) so macOS LaunchServices treats Lumen as a distinct app.
  # Binary file name stays `warp-oss` to avoid touching the Cargo bin
  # target rename surface.
  "$SCRIPT_DIR/rename-to-lumen.patch"
)
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

# Apply each patch idempotently. `--reverse --check` succeeds when the
# patch is already in the tree; in that case skip. Otherwise validate +
# apply forward. Patches are applied in order; one failure aborts.
for PATCH_FILE in "${PATCH_FILES[@]}"; do
  patch_name=$(basename "$PATCH_FILE")
  echo "==> Applying $patch_name"
  if git -C "$UPSTREAM_DIR" apply --reverse --check "$PATCH_FILE" 2>/dev/null; then
    echo "    (already applied, skipping)"
  else
    git -C "$UPSTREAM_DIR" apply --check "$PATCH_FILE"
    git -C "$UPSTREAM_DIR" apply "$PATCH_FILE"
  fi
done

# Build the OSS binary in release mode. By default uses Metal; pass
# WARP_OSS_USE_WGPU=1 to opt into the wgpu renderer (Metal-bypass) for
# machines where the Metal Toolchain is unavailable.
echo "==> Building warp-oss (release)"
if [ "${WARP_OSS_USE_WGPU:-0}" = "1" ]; then
  echo "    (using experimental-wgpu-renderer — Metal-bypass)"
  ( cd "$UPSTREAM_DIR" && cargo build --release --bin warp-oss \
    --features warpui/experimental-wgpu-renderer )
else
  ( cd "$UPSTREAM_DIR" && cargo build --release --bin warp-oss )
fi

# Resolve the binary path. The host workspace (life worktree) sets
# `target-dir = ".target"` in `.cargo/config.toml`, which cargo inherits
# when run from any subdirectory — so the actual binary may land in the
# shared `.target/release/` rather than the per-subworkspace path.
# Check both; report whichever exists.
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIN_CANDIDATES=(
  "$WORKSPACE_ROOT/.target/release/warp-oss"
  "$UPSTREAM_DIR/target/release/warp-oss"
)
BIN=""
for c in "${BIN_CANDIDATES[@]}"; do
  if [ -x "$c" ]; then BIN="$c"; break; fi
done
if [ -n "$BIN" ]; then
  echo "==> Built: $BIN"
else
  echo "ERROR: expected binary not found in any of:" >&2
  printf '  - %s\n' "${BIN_CANDIDATES[@]}" >&2
  exit 1
fi
