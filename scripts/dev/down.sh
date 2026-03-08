#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
AIOS_STATE_ROOT="${AIOS_STATE_ROOT:-/home/exedev/.aios}"
RUNTIME_PID_DIR="$AIOS_STATE_ROOT/runtime/pids"

log() { echo "[down] $*"; }

stop_service() {
  local name="$1"
  local pidfile="$RUNTIME_PID_DIR/${name}.pid"
  if [[ ! -f "$pidfile" ]]; then
    log "$name not running (no pidfile)"
    return 0
  fi
  local pid
  pid="$(cat "$pidfile" 2>/dev/null || true)"
  if [[ -z "$pid" ]]; then
    rm -f "$pidfile"
    log "$name pidfile empty, cleaned"
    return 0
  fi

  if kill -0 "$pid" 2>/dev/null; then
    log "stopping $name (pid=$pid)"
    kill "$pid" 2>/dev/null || true
    sleep 1
    kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null || true
  else
    log "$name process already not running"
  fi
  rm -f "$pidfile"
}

stop_service arcan
stop_service autonomicd
stop_service lagod

log "done"
