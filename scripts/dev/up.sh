#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1 && [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1090
  . "$HOME/.cargo/env"
fi

AIOS_STATE_ROOT="${AIOS_STATE_ROOT:-/home/exedev/.aios}"
TENANT_ID="${AIOS_TENANT_ID:-default}"
PROJECT_ID="${AIOS_PROJECT_ID:-life}"
SESSION_ID="${AIOS_SESSION_ID:-dev}"

RUNTIME_ROOT="$AIOS_STATE_ROOT/runtime"
RUNTIME_LOG_DIR="$RUNTIME_ROOT/logs"
RUNTIME_PID_DIR="$RUNTIME_ROOT/pids"
RUNTIME_SOCKET_DIR="$RUNTIME_ROOT/sockets"
SESSION_ROOT="$AIOS_STATE_ROOT/tenants/$TENANT_ID/projects/$PROJECT_ID/sessions/$SESSION_ID"
CONTROL_STATE_DIR="$AIOS_STATE_ROOT/control/state"

mkdir -p "$RUNTIME_LOG_DIR" "$RUNTIME_PID_DIR" "$RUNTIME_SOCKET_DIR" "$SESSION_ROOT" "$CONTROL_STATE_DIR"

ARCAN_PORT="${ARCAN_PORT:-3000}"
AUTONOMIC_PORT="${AUTONOMIC_PORT:-3002}"

SKIP_BUILD=false
NO_LAGO=false
NO_AUTONOMIC=false
NO_ARCAN=false

usage() {
  cat <<EOF
Usage: scripts/dev/up.sh [options]

Options:
  --skip-build      Skip pre-build checks for service binaries
  --no-lago         Do not start lagod
  --no-autonomic    Do not start autonomicd
  --no-arcan        Do not start arcan
  -h, --help        Show help

Environment:
  AIOS_STATE_ROOT   Canonical unified state root (default: /home/exedev/.aios)
  AIOS_TENANT_ID    Tenant namespace (default: default)
  AIOS_PROJECT_ID   Project namespace (default: life)
  AIOS_SESSION_ID   Session namespace (default: dev)
  ARCAN_PORT        Default 3000
  AUTONOMIC_PORT    Default 3002
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=true; shift ;;
    --no-lago) NO_LAGO=true; shift ;;
    --no-autonomic) NO_AUTONOMIC=true; shift ;;
    --no-arcan) NO_ARCAN=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage; exit 1 ;;
  esac
done

log() { echo "[up] $*"; }
warn() { echo "[up][warn] $*"; }

is_running() {
  local pidfile="$1"
  [[ -f "$pidfile" ]] || return 1
  local pid
  pid="$(cat "$pidfile" 2>/dev/null || true)"
  [[ -n "$pid" ]] || return 1
  kill -0 "$pid" 2>/dev/null
}

start_service() {
  local name="$1"; shift
  local workdir="$1"; shift
  local cmd="$*"
  local pidfile="$RUNTIME_PID_DIR/${name}.pid"
  local logfile="$RUNTIME_LOG_DIR/${name}.log"

  if is_running "$pidfile"; then
    log "$name already running (pid=$(cat "$pidfile"))"
    return 0
  fi

  log "starting $name ..."
  (
    cd "$workdir"
    nohup bash -lc "$cmd" >> "$logfile" 2>&1 &
    echo $! > "$pidfile"
  )
  sleep 1

  if is_running "$pidfile"; then
    log "$name started (pid=$(cat "$pidfile"), log=$logfile)"
  else
    warn "$name failed to start; check $logfile"
  fi
}

wait_http() {
  local name="$1"; local url="$2"; local timeout_s="${3:-20}"
  local i=0
  while (( i < timeout_s )); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      log "$name healthy at $url"
      return 0
    fi
    sleep 1
    ((i+=1))
  done
  warn "$name health endpoint not reachable yet: $url"
  return 1
}

if ! $SKIP_BUILD; then
  log "pre-building service binaries (fast-fail) ..."
  $NO_LAGO || (cd lago && cargo build -p lagod --quiet)
  $NO_AUTONOMIC || (cd autonomic && cargo build -p autonomicd --quiet)
  $NO_ARCAN || (cd arcan && cargo build -p arcan --quiet)
  log "build step completed"
fi

$NO_LAGO || start_service "lagod" "$ROOT/lago" "cargo run -p lagod -- --data-dir '$SESSION_ROOT'"
$NO_AUTONOMIC || start_service "autonomicd" "$ROOT/autonomic" "cargo run -p autonomicd -- --lago-data-dir '$CONTROL_STATE_DIR'"
$NO_ARCAN || start_service "arcan" "$ROOT/arcan" "cargo run -p arcan -- --data-dir '$SESSION_ROOT'"

# Best-effort health checks (non-fatal; different binaries may expose different routes)
$NO_ARCAN || wait_http "arcan" "http://127.0.0.1:${ARCAN_PORT}/health" 25 || true
$NO_AUTONOMIC || wait_http "autonomicd" "http://127.0.0.1:${AUTONOMIC_PORT}/health" 20 || true

echo
log "AIOS state root: $AIOS_STATE_ROOT"
log "session root: $SESSION_ROOT"
log "runtime logs: $RUNTIME_LOG_DIR"
log "runtime pids: $RUNTIME_PID_DIR"
log "next commands:"
echo "  - stop all:  $ROOT/scripts/dev/down.sh"
echo "  - show pids: ls -la $RUNTIME_PID_DIR/*.pid"
echo "  - tail logs: tail -f $RUNTIME_LOG_DIR/arcan.log"
echo "  - control gates: make smoke && make check && make test"
