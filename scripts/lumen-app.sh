#!/usr/bin/env bash
# Lumen Phase α — agent-driven GUI interaction harness for the patched warp-oss binary.
#
# Goal: give the running agent (Claude, codex, etc.) the same empirical loop
# a human developer has — launch the app, drive UI inputs, capture state,
# tail logs — so changes to lifegw/lifed/lumen can be validated end-to-end.
#
# This is the missing piece that lifts Phase α from "wire-protocol smoke is
# green" to "the actual GUI app produces the expected screen states when
# wired to Life."
#
# Composes with the existing harness:
#   - scripts/lumen-smoke.sh    → brings up lifegw + lifed (the backend)
#   - scripts/lumen-app.sh      → drives the warp-oss GUI client (THIS FILE)
#   - examples/lumen_smoke_client.rs → headless wire-protocol smoke
#
# Subcommands:
#   build [--wgpu]         Build the patched warp-oss binary. --wgpu opts into
#                          the experimental-wgpu-renderer fallback when the
#                          Metal Toolchain is unavailable.
#   launch [--bg]          Spawn warp-oss with Life env vars wired up. --bg
#                          backgrounds the process (default: foreground until
#                          ready, then daemonize).
#   pid                    Show running warp-oss PID (or empty).
#   windows                List warp-oss window IDs + titles (System Events).
#   shot [PATH]            Screenshot the warp-oss app to PATH (default: a
#                          timestamped file under docs/lumen/validation/...).
#   type "TEXT"            Type TEXT as keystrokes into the frontmost warp-oss
#                          window (after `activate`).
#   key NAME               Send a named key: return, tab, escape, down, up,
#                          left, right, space, delete.
#   chord "CMD+KEY"        Send a modified key combo, e.g. "cmd+k", "cmd+t".
#   click X Y              Click at absolute screen coords (CGEvent via cliclick
#                          if installed, else osascript fallback).
#   logs [tail|grep PAT]   Unified view of lifegw.log + lifed.log. `tail`
#                          follows; `grep PAT` searches.
#   wait "TEXT" [SECS]     Block until "TEXT" appears in lifegw.log (default
#                          timeout 30s). Returns 0 on hit, 1 on timeout.
#   probe                  Quick health probe: PIDs + window count + last
#                          log line. Designed for agent self-orientation.
#   down                   Stop warp-oss cleanly (SIGTERM, then SIGKILL after
#                          3s if still alive).
#   loop                   End-to-end demo: build (if needed) → ensure stack →
#                          launch → wait for window → drive a sign-in flow →
#                          screenshot each stage. Designed as the canonical
#                          "validate everything works" command.
#
# Design notes (P14 dep chain considerations):
#   - This script depends ONLY on macOS built-ins (osascript, screencapture,
#     pgrep). No Homebrew prerequisites. `cliclick` is optionally probed for
#     more reliable clicking but a pure-osascript fallback works.
#   - All evidence lives under docs/lumen/validation/$(date -u +%Y-%m-%d)/
#     so a single git add captures the full agent run.
#   - The patched binary's process name is "warp-oss" (matches the cargo
#     bin name in upstream/Cargo.toml). osascript targets that exact name.
#   - launch() reuses scripts/lumen-smoke.sh's env-var contract so the GUI
#     client speaks to the same lifegw/lifed pair the headless smoke does.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WARP_OSS_DIR="$REPO_ROOT/apps/warp-oss-patch/upstream"
# The worktree's .cargo/config.toml redirects `target-dir` to `.target/` at
# the workspace root, so the produced binary lives there — not under the
# warp-oss-patch subdir. Resolve both candidates and pick whichever exists.
_warp_oss_bin_candidates=(
  "$REPO_ROOT/.target/release/warp-oss"
  "$WARP_OSS_DIR/target/release/warp-oss"
)
WARP_OSS_BIN=""
for _c in "${_warp_oss_bin_candidates[@]}"; do
  if [ -x "$_c" ]; then WARP_OSS_BIN="$_c"; break; fi
done
# Default to the shared-target path for build-time messaging when neither
# exists yet.
WARP_OSS_BIN="${WARP_OSS_BIN:-${_warp_oss_bin_candidates[0]}}"
WORKDIR="/tmp/lumen-app"
LIFEGW_LOG="/tmp/lumen-smoke/lifegw.log"
LIFED_LOG="/tmp/lumen-smoke/lifed.log"
APP_LOG="$WORKDIR/warp-oss.log"
APP_PIDFILE="$WORKDIR/warp-oss.pid"
PROC_NAME="warp-oss"
# Minimal .app wrapper around the bare binary. Running the bare Mach-O
# directly leaves macOS treating the process as a background app (no
# NSApplication activation, no AppleEvent responses, no main window).
# Wrapping the binary in a .app bundle with an Info.plist (CFBundleExecutable,
# CFBundlePackageType=APPL, LSBackgroundOnly=false, NSPrincipalClass=NSApplication)
# gives the OS the metadata it needs to wire the GUI lifecycle correctly.
#
# Lumen Phase α (broomva): the bundle is named `Lumen.app` and uses
# `dev.broomva.Lumen` as its identifier so LaunchServices treats the OSS
# build as a distinct macOS app from the user's installed Warp.app
# (stable). This matches the AppId rename baked into the binary by
# `rename-to-lumen.patch` (which controls the Application Support
# subdir under the team-level Group Container, isolating SQLite +
# keychain + secure storage).
APP_BUNDLE="$WORKDIR/Lumen.app"

# Default Life endpoint contract (matches scripts/lumen-smoke.sh).
WARP_SERVER_ROOT_URL="${WARP_SERVER_ROOT_URL:-https://127.0.0.1:8443}"
WARP_WS_SERVER_URL="${WARP_WS_SERVER_URL:-wss://127.0.0.1:8443/graphql-ws}"
WARP_AUTH_TOKEN="${WARP_AUTH_TOKEN:-dev-cap-token-for-lumen/test-user-1}"

C_INFO=$'\e[1;36m'
C_OK=$'\e[1;32m'
C_WARN=$'\e[1;33m'
C_ERR=$'\e[1;31m'
C_DIM=$'\e[2m'
C_RST=$'\e[0m'

info()  { printf "${C_INFO}[lumen-app]${C_RST} %s\n" "$*"; }
ok()    { printf "${C_OK}[lumen-app]${C_RST} %s\n" "$*"; }
warn()  { printf "${C_WARN}[lumen-app]${C_RST} %s\n" "$*"; }
err()   { printf "${C_ERR}[lumen-app]${C_RST} %s\n" "$*" >&2; }
dim()   { printf "${C_DIM}%s${C_RST}\n" "$*"; }

ensure_workdir() {
  mkdir -p "$WORKDIR"
}

evidence_dir() {
  local d="$REPO_ROOT/docs/lumen/validation/$(date -u +%Y-%m-%d)"
  mkdir -p "$d"
  printf '%s' "$d"
}

# ── build ────────────────────────────────────────────────────────────────────

cmd_build() {
  local wgpu=0
  for arg in "$@"; do
    case "$arg" in
      --wgpu) wgpu=1 ;;
      *) err "unknown flag: $arg"; return 2 ;;
    esac
  done

  if [ ! -d "$WARP_OSS_DIR/.git" ]; then
    info "Cloning + patching warp-oss upstream"
    "$REPO_ROOT/apps/warp-oss-patch/apply-patch.sh"
    return $?
  fi

  cd "$WARP_OSS_DIR"

  if [ "$wgpu" -eq 1 ]; then
    info "Building warp-oss with experimental-wgpu-renderer (Metal-bypass)"
    # The build.rs patch applied by apply-patch.sh makes the metal-shader
    # compile conditional on the feature; with experimental-wgpu-renderer
    # active, the metal-shader path is skipped.
    cargo build --release --bin warp-oss --features warpui/experimental-wgpu-renderer
  else
    info "Building warp-oss with default (Metal) renderer"
    cargo build --release --bin warp-oss
  fi

  if [ -x "$WARP_OSS_BIN" ]; then
    ok "Binary: $WARP_OSS_BIN ($(du -h "$WARP_OSS_BIN" | awk '{print $1}'))"
  else
    err "Build claimed success but $WARP_OSS_BIN is missing"
    return 1
  fi
}

# ── launch / down / pid ──────────────────────────────────────────────────────

cmd_pid() {
  if [ -f "$APP_PIDFILE" ]; then
    local pid
    pid=$(cat "$APP_PIDFILE")
    if kill -0 "$pid" 2>/dev/null; then
      printf '%s\n' "$pid"
      return 0
    fi
    rm -f "$APP_PIDFILE"
  fi
  # Fallback: pgrep
  pgrep -x "$PROC_NAME" 2>/dev/null | head -1 || true
}

ensure_app_bundle() {
  # Idempotently produce a minimal .app wrapper around $WARP_OSS_BIN so
  # macOS treats it as a foreground GUI app. Cheap to recreate every
  # launch; skips work if everything is already wired.
  if [ ! -x "$WARP_OSS_BIN" ]; then
    err "Binary not built: $WARP_OSS_BIN"
    err "Run: $0 build  (or $0 build --wgpu if Metal Toolchain unavailable)"
    return 1
  fi
  mkdir -p "$APP_BUNDLE/Contents/MacOS" "$APP_BUNDLE/Contents/Resources"
  # Replace the symlink unconditionally so a rebuilt binary is always reflected.
  ln -sf "$WARP_OSS_BIN" "$APP_BUNDLE/Contents/MacOS/warp-oss"
  if [ ! -f "$APP_BUNDLE/Contents/Info.plist" ]; then
    cat > "$APP_BUNDLE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Lumen</string>
  <key>CFBundleExecutable</key>
  <string>warp-oss</string>
  <key>CFBundleIdentifier</key>
  <string>dev.broomva.Lumen</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Lumen</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.0.0</string>
  <key>CFBundleVersion</key>
  <string>0</string>
  <key>LSMinimumSystemVersion</key>
  <string>10.14</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>LSBackgroundOnly</key>
  <false/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
</dict>
</plist>
PLIST
  fi
}

# Phase α₁ — bootstrap a Lumen Tier-User cap from lifegw and stash it
# so the launcher can inject it via `WARP_USER_SECRET`. Lumen's upstream
# `app/src/auth/auth_state.rs:133-138` reads that env var at startup and
# wraps it as `Credentials::ApiKey` (or `Bearer` depending on prefix),
# attaching it as `Authorization: Bearer <token>` on every HTTPS call.
# This is what removes the Firebase / OAuth-via-browser leg of the
# auth flow — there is no longer a `/login/remote` browser handoff.
#
# Returns the token on stdout, or non-zero on failure. The caller is
# expected to capture stdout into a variable.
#
# Architecture context: `docs/lumen/architecture/2026-05-15-life-native-wiring.md`
# §"Phase α₁ in detail". Endpoint contract:
# `crates/life-runtime/lifegw/src/services/auth_bootstrap.rs`.
lumen_bootstrap_token() {
  local user_id="${1:-lumen-local}"
  local endpoint="${WARP_SERVER_ROOT_URL%/}/life/v1/auth/bootstrap"
  # `--cacert dev-tls/dev-ca.pem` is the cleanest: we pin the local CA
  # we generated via `scripts/dev-tls-bootstrap.sh` rather than relying
  # on `-k` (which would also work since the user trusted the CA in
  # the system keychain, but pinning makes the script self-contained
  # for callers running outside the macOS keychain context).
  local ca_pem="$REPO_ROOT/crates/life-runtime/lifegw/dev-tls/dev-ca.pem"
  local cacert_flag=""
  if [ -f "$ca_pem" ]; then
    cacert_flag="--cacert $ca_pem"
  else
    cacert_flag="-k"
  fi
  local response
  response=$(curl -sS $cacert_flag -X POST "$endpoint" \
    -H "Content-Type: application/json" \
    -d "{\"user_id\":\"$user_id\",\"client_label\":\"Lumen GUI\"}" 2>&1) || {
    err "bootstrap call to $endpoint failed: $response"
    return 1
  }
  local token
  token=$(printf '%s' "$response" \
    | python3 -c 'import json,sys; print(json.loads(sys.stdin.read()).get("access_token",""))' 2>/dev/null)
  if [ -z "$token" ]; then
    err "bootstrap response missing access_token; raw: $response"
    return 1
  fi
  printf '%s' "$token"
}

cmd_launch() {
  ensure_workdir
  ensure_app_bundle || return 1

  local existing
  existing=$(cmd_pid)
  if [ -n "$existing" ]; then
    warn "warp-oss already running (PID $existing) — use \`$0 down\` first"
    return 1
  fi

  # Phase α₁: mint a Tier-User cap from lifegw and stash it. If the
  # mint fails, fall back to the legacy dev-cap-token shortcut (this
  # keeps the launcher useful for testing the wire even when lifegw
  # is down). The dev-cap-token shortcut path was the only auth
  # plumbing available pre-Phase α₁.
  local user_secret
  if user_secret=$(lumen_bootstrap_token "lumen-local" 2>/dev/null) && [ -n "$user_secret" ]; then
    ok "minted Tier-User cap from lifegw (${#user_secret} chars)"
  else
    warn "lifegw bootstrap mint failed; falling back to dev-cap-token shortcut"
    user_secret="$WARP_AUTH_TOKEN"
  fi

  info "Launching Lumen.app"
  dim "  WARP_SERVER_ROOT_URL = $WARP_SERVER_ROOT_URL"
  dim "  WARP_WS_SERVER_URL   = $WARP_WS_SERVER_URL"
  dim "  WARP_AUTH_TOKEN      = $WARP_AUTH_TOKEN  (legacy shortcut)"
  dim "  WARP_USER_SECRET     = ${user_secret:0:40}...  (Phase α₁ Tier-User cap)"
  dim "  app bundle           = $APP_BUNDLE"
  dim "  log file             = $APP_LOG"

  # Launch via `open` so macOS LaunchServices wires the .app bundle's
  # Info.plist into NSApplication. `open -a PATH` activates the bundle;
  # `--stdout`/`--stderr` route the binary's fds to our log file. We
  # pass `-W` no because we want the call to return after launch (the
  # app keeps running in its own process).
  #
  # We pass env vars through `--env`; LaunchServices doesn't inherit
  # the calling shell's environment, so the bare `--env KEY=VALUE`
  # form is the only reliable way to thread the Life endpoints in.
  #
  # `WARP_USER_SECRET` is the Phase α₁ Tier-User cap we minted above —
  # Lumen reads it at `app/src/auth/auth_state.rs:133-138` and wraps as
  # `Credentials::Bearer`, attaching it to every HTTPS/WS call. This is
  # what bypasses Firebase + the OAuth-via-browser flow entirely.
  WARP_SERVER_ROOT_URL="$WARP_SERVER_ROOT_URL" \
  WARP_WS_SERVER_URL="$WARP_WS_SERVER_URL" \
  WARP_AUTH_TOKEN="$WARP_AUTH_TOKEN" \
  WARP_USER_SECRET="$user_secret" \
  RUST_LOG="${RUST_LOG:-warp_oss=debug,info}" \
  open --new --background \
    --stdout "$APP_LOG" --stderr "$APP_LOG" \
    --env WARP_SERVER_ROOT_URL="$WARP_SERVER_ROOT_URL" \
    --env WARP_WS_SERVER_URL="$WARP_WS_SERVER_URL" \
    --env WARP_AUTH_TOKEN="$WARP_AUTH_TOKEN" \
    --env WARP_USER_SECRET="$user_secret" \
    --env RUST_LOG="${RUST_LOG:-warp_oss=debug,info}" \
    "$APP_BUNDLE"

  # Wait up to ~5s for the process to register with launchd + acquire
  # a PID under our bundle ID. Polling on pgrep is the cheapest path.
  local pid="" tries=0
  while [ -z "$pid" ] && [ "$tries" -lt 50 ]; do
    pid=$(pgrep -x warp-oss 2>/dev/null | head -1 || true)
    tries=$((tries + 1))
    sleep 0.1
  done
  if [ -z "$pid" ]; then
    err "warp-oss did not register a PID within 5 s — see $APP_LOG"
    tail -30 "$APP_LOG" >&2 || true
    return 1
  fi
  echo "$pid" > "$APP_PIDFILE"

  # Give it ~3 s to fully initialise NSApplication.
  sleep 3
  if ! kill -0 "$pid" 2>/dev/null; then
    err "warp-oss exited within 3 s — see $APP_LOG"
    tail -30 "$APP_LOG" >&2 || true
    rm -f "$APP_PIDFILE"
    return 1
  fi
  ok "warp-oss running (PID $pid)"
}

cmd_down() {
  local pid
  pid=$(cmd_pid)
  if [ -z "$pid" ]; then
    info "warp-oss not running"
    return 0
  fi
  info "Stopping warp-oss (PID $pid)"
  kill -TERM "$pid" 2>/dev/null || true
  for _ in 1 2 3; do
    sleep 1
    if ! kill -0 "$pid" 2>/dev/null; then break; fi
  done
  if kill -0 "$pid" 2>/dev/null; then
    warn "SIGTERM ignored — SIGKILL"
    kill -KILL "$pid" 2>/dev/null || true
  fi
  rm -f "$APP_PIDFILE"
  ok "stopped"
}

# ── windows / shot / drive ───────────────────────────────────────────────────

cmd_windows() {
  if [ -z "$(cmd_pid)" ]; then
    warn "warp-oss not running"
    return 1
  fi
  osascript <<'AS'
tell application "System Events"
  if exists process "warp-oss" then
    set out to {}
    repeat with w in (windows of process "warp-oss")
      set wid to id of w
      set wname to name of w
      copy ((wid as string) & "  " & wname) to end of out
    end repeat
    return my joinList(out, linefeed)
  else
    return "no warp-oss process"
  end if
end tell

on joinList(lst, sep)
  set AppleScript's text item delimiters to sep
  set s to lst as text
  set AppleScript's text item delimiters to ""
  return s
end joinList
AS
}

cmd_shot() {
  local out="${1:-}"
  if [ -z "$out" ]; then
    out="$(evidence_dir)/screenshot-$(date -u +%H%M%S).png"
  fi
  mkdir -p "$(dirname "$out")"
  if [ -z "$(cmd_pid)" ]; then
    warn "warp-oss not running — capturing full screen instead"
    /usr/sbin/screencapture -x "$out"
  else
    # Focus warp-oss, then capture its front window.
    osascript -e 'tell application "warp-oss" to activate' 2>/dev/null || true
    sleep 0.4
    # -l <wid> captures a specific window by CGWindowID; we ask System Events
    # for the front window id. If that fails, fall back to full-screen
    # screenshot of the active display.
    local wid
    wid=$(osascript -e 'tell application "System Events" to id of front window of process "warp-oss"' 2>/dev/null || true)
    if [[ "$wid" =~ ^[0-9]+$ ]]; then
      /usr/sbin/screencapture -x -l "$wid" "$out"
    else
      warn "no front window id — full-screen capture"
      /usr/sbin/screencapture -x "$out"
    fi
  fi
  if [ -s "$out" ]; then
    ok "screenshot → $out"
    printf '%s\n' "$out"
  else
    err "screenshot empty or missing: $out"
    return 1
  fi
}

cmd_type() {
  local text="${1:?need text to type}"
  osascript -e 'tell application "warp-oss" to activate' 2>/dev/null || true
  sleep 0.3
  # Escape backslashes and double-quotes for AppleScript string literal.
  local escaped=${text//\\/\\\\}
  escaped=${escaped//\"/\\\"}
  osascript -e "tell application \"System Events\" to keystroke \"$escaped\""
}

cmd_key() {
  local name="${1:?need key name}"
  local code
  case "$name" in
    return|enter) code=36 ;;
    tab)          code=48 ;;
    escape|esc)   code=53 ;;
    space)        code=49 ;;
    delete|bs)    code=51 ;;
    up)           code=126 ;;
    down)         code=125 ;;
    left)         code=123 ;;
    right)        code=124 ;;
    *) err "unknown key: $name"; return 2 ;;
  esac
  osascript -e 'tell application "warp-oss" to activate' 2>/dev/null || true
  sleep 0.2
  osascript -e "tell application \"System Events\" to key code $code"
}

cmd_chord() {
  local combo="${1:?need chord like cmd+k}"
  # combo format: mod[+mod...]+key  e.g. cmd+k, cmd+shift+t, cmd+option+i
  local IFS=+
  read -r -a parts <<<"$combo"
  local mods=()
  local key=""
  for p in "${parts[@]}"; do
    case "${p,,}" in
      cmd|command)   mods+=("command down") ;;
      shift)         mods+=("shift down") ;;
      opt|option|alt) mods+=("option down") ;;
      ctrl|control)  mods+=("control down") ;;
      *) key="$p" ;;
    esac
  done
  if [ -z "$key" ]; then err "no key in chord '$combo'"; return 2; fi
  local mods_clause=""
  if [ "${#mods[@]}" -gt 0 ]; then
    mods_clause=" using {$(IFS=','; echo "${mods[*]}")}"
  fi
  osascript -e 'tell application "warp-oss" to activate' 2>/dev/null || true
  sleep 0.2
  osascript -e "tell application \"System Events\" to keystroke \"$key\"$mods_clause"
}

cmd_click() {
  local x="${1:?need x}"
  local y="${2:?need y}"
  if command -v cliclick >/dev/null 2>&1; then
    cliclick "c:$x,$y"
  else
    # osascript click via System Events — works but is async; allow a tick.
    osascript -e "tell application \"System Events\" to click at {$x, $y}"
  fi
  ok "clicked ($x, $y)"
}

# ── logs / wait / probe ──────────────────────────────────────────────────────

cmd_logs() {
  local mode="${1:-tail}"
  case "$mode" in
    tail)
      info "Unified tail of lifegw.log + lifed.log + warp-oss.log (Ctrl-C to exit)"
      tail -F "$LIFEGW_LOG" "$LIFED_LOG" "$APP_LOG" 2>/dev/null
      ;;
    grep)
      local pat="${2:?need grep pattern}"
      info "grep '$pat' across the three logs"
      grep -nE "$pat" "$LIFEGW_LOG" "$LIFED_LOG" "$APP_LOG" 2>/dev/null || true
      ;;
    *) err "logs: unknown mode '$mode' (use tail|grep PAT)"; return 2 ;;
  esac
}

cmd_wait() {
  local text="${1:?need text to wait for}"
  local timeout="${2:-30}"
  info "waiting for '$text' in lifegw.log (timeout ${timeout}s)"
  local deadline=$(( $(date +%s) + timeout ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if grep -q -F -- "$text" "$LIFEGW_LOG" 2>/dev/null; then
      ok "saw '$text'"
      return 0
    fi
    sleep 0.5
  done
  err "timeout waiting for '$text'"
  return 1
}

cmd_probe() {
  local pid wins last
  pid=$(cmd_pid)
  if [ -n "$pid" ]; then
    info "warp-oss: RUNNING (PID $pid)"
    wins=$(cmd_windows 2>/dev/null | wc -l | tr -d ' ')
    dim "  windows: $wins"
  else
    info "warp-oss: not running"
  fi
  if [ -f "$LIFEGW_LOG" ]; then
    last=$(tail -1 "$LIFEGW_LOG" 2>/dev/null || echo "(empty)")
    dim "  last lifegw.log line: $last"
  fi
  if [ -f "$APP_LOG" ]; then
    last=$(tail -1 "$APP_LOG" 2>/dev/null || echo "(empty)")
    dim "  last warp-oss.log line: $last"
  fi
}

# ── orchestrated loop ────────────────────────────────────────────────────────

cmd_loop() {
  ensure_workdir
  info "=== Lumen agent-driven validation loop ==="
  # 1. Ensure backend.
  if ! "$REPO_ROOT/scripts/lumen-smoke.sh" status 2>&1 | grep -q "RUNNING"; then
    info "Backend not up — bringing up lifegw + lifed"
    "$REPO_ROOT/scripts/lumen-smoke.sh" up >/dev/null 2>&1 &
    sleep 30  # give the stack time to bind
  fi
  # 2. Ensure binary.
  if [ ! -x "$WARP_OSS_BIN" ]; then
    err "Binary missing. Run: $0 build [--wgpu]"
    return 1
  fi
  # 3. Launch.
  cmd_launch
  # 4. Wait for the first WS subscribe from the real app.
  cmd_wait "graphql-ws" 30 || warn "no WS handshake yet — app may still be on splash"
  # 5. Snapshot the initial state.
  local d="$(evidence_dir)"
  cmd_shot "$d/loop-1-launch.png"
  # 6. Drive: focus the chat input + send a test prompt.
  cmd_chord "cmd+k" || true   # warp's command palette toggle (best-effort)
  sleep 0.5
  cmd_key escape || true
  cmd_type "echo hello from lumen agent loop"
  sleep 0.5
  cmd_shot "$d/loop-2-typed.png"
  cmd_key return
  sleep 2
  cmd_shot "$d/loop-3-after-return.png"
  ok "Loop complete — evidence under $d/"
}

# ── entry ────────────────────────────────────────────────────────────────────

usage() {
  sed -n '1,80p' "$0" | sed -n '/^# Subcommands:/,/^$/p'
}

cmd="${1:-}"
shift || true
case "$cmd" in
  build)    cmd_build "$@" ;;
  launch)   cmd_launch "$@" ;;
  pid)      cmd_pid "$@" ;;
  windows)  cmd_windows "$@" ;;
  shot)     cmd_shot "$@" ;;
  type)     cmd_type "$@" ;;
  key)      cmd_key "$@" ;;
  chord)    cmd_chord "$@" ;;
  click)    cmd_click "$@" ;;
  logs)     cmd_logs "$@" ;;
  wait)     cmd_wait "$@" ;;
  probe)    cmd_probe "$@" ;;
  down)     cmd_down "$@" ;;
  loop)     cmd_loop "$@" ;;
  ""|-h|--help|help) usage ;;
  *) err "unknown subcommand: $cmd"; usage; exit 2 ;;
esac
