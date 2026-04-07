#!/usr/bin/env bash
set -euo pipefail

# Control heartbeat: environment-first health validation for autonomous development.

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT_DIR"

failures=0
alerts=()

ok() { echo "[ok] $1"; }
err() { echo "[fail] $1"; failures=$((failures + 1)); alerts+=("$1"); }

check_cmd() {
  local name="$1"
  if command -v "$name" >/dev/null 2>&1; then
    ok "binary available: $name"
  else
    err "missing binary: $name (environment remediation required)"
  fi
}

echo "== heartbeat: environment prerequisites =="
check_cmd git
check_cmd make
check_cmd cargo
check_cmd jq

echo "== heartbeat: host capacity =="
avail_kb=$(df -Pk . | awk 'NR==2{print $4}')
# 2 GiB minimum free space guard
if [ "${avail_kb:-0}" -ge 2097152 ]; then
  ok "disk headroom OK ($(df -h . | awk 'NR==2{print $4" free"}'))"
else
  err "low disk headroom (<2GiB free); cleanup required before heavy builds"
fi

echo "== heartbeat: repo control invariants =="
if [ -f .life/control/policy.yaml ] && [ -f .life/control/commands.yaml ] && [ -f .life/control/topology.yaml ]; then
  ok "control policy artifacts present"
else
  err "control policy artifacts missing"
fi

hooks_path=$(git config --get core.hooksPath || true)
if [ "$hooks_path" = ".githooks" ]; then
  ok "git hooks path configured"
else
  err "git hooks path not configured (.githooks expected)"
fi

echo "== heartbeat: control feedback loop =="
if ./scripts/audit_control.sh . >/tmp/control-heartbeat-audit.log 2>&1; then
  ok "control audit passed"
else
  err "control audit failed"
  cat /tmp/control-heartbeat-audit.log
fi

if ./scripts/audit_harness.sh . >/tmp/control-heartbeat-harness.log 2>&1; then
  ok "harness audit passed"
else
  err "harness audit failed"
  cat /tmp/control-heartbeat-harness.log
fi

if [ "$failures" -eq 0 ]; then
  echo "HEARTBEAT_OK"
  exit 0
fi

echo "HEARTBEAT_ALERT: ${alerts[*]}"
exit 1
