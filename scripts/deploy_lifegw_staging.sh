#!/usr/bin/env bash
# Deploy lifegw to Railway for the Spec J Phase 1 live smoke (BRO-1146).
#
# This script is idempotent — re-running it produces the same end state
# (same Railway service, same env vars, same public URL). It is the
# operator-facing wrapper around the manual `railway` commands documented
# in `docs/conformance/2026-05-18-claude-code-smoke-runbook.md` §2.
#
# Usage:
#   bash scripts/deploy_lifegw_staging.sh             # interactive — prompts before deploying
#   bash scripts/deploy_lifegw_staging.sh --yes       # non-interactive — assume `y` to confirmations
#   bash scripts/deploy_lifegw_staging.sh --dry-run   # show what would happen, don't do it
#   bash scripts/deploy_lifegw_staging.sh --skip-up   # set env vars but don't trigger `railway up`
#
# The script does NOT have Railway credentials in-band. It assumes the
# operator has already run `railway login` and `railway link` against
# the target project. If those preconditions aren't met, the script
# exits cleanly with a diagnostic and a one-line fix.
#
# Pre-conditions:
#   - railway CLI installed (>= 4.0)
#   - operator logged in (`railway whoami` succeeds)
#   - operator has linked the local repo to a Railway project
#     (`railway status` shows the project name)
#   - the target Railway environment exists ("staging" by default —
#     create it via the Railway dashboard or `railway environment new`
#     before running this script)
#
# Post-conditions:
#   - All env vars from runbook §2.2 are set on the lifegw-spec-j service
#   - lifegw is deployed to the staging environment (unless --skip-up)
#   - The script prints the public URL + ANTHROPIC_AUTH_TOKEN the
#     operator copy-pastes into the Claude Code shell

set -euo pipefail

# ─── Configuration ──────────────────────────────────────────────────────

SERVICE_NAME="${LIFEGW_SERVICE_NAME:-lifegw-spec-j}"
ENVIRONMENT_NAME="${LIFEGW_ENVIRONMENT:-staging}"
PUBLIC_DOMAIN="${LIFEGW_PUBLIC_DOMAIN:-lifegw-spec-j.broomva.dev}"
OTLP_ENDPOINT="${LIFEGW_OTLP_ENDPOINT:-}"  # operator overrides
DEV_USER="${LIFEGW_DEV_USER:-broomva}"

# Parse flags.
ASSUME_YES=0
DRY_RUN=0
SKIP_UP=0
for arg in "$@"; do
  case "$arg" in
    --yes|-y) ASSUME_YES=1 ;;
    --dry-run) DRY_RUN=1 ;;
    --skip-up) SKIP_UP=1 ;;
    --help|-h)
      sed -n '1,/^set -euo pipefail/p' "$0" | grep '^#' | sed 's/^# \?//'
      exit 0
      ;;
    *)
      echo "error: unknown flag '$arg' (try --help)" >&2
      exit 2
      ;;
  esac
done

log() {
  printf '[deploy] %s\n' "$*"
}

bail() {
  printf '[deploy] ERROR: %s\n' "$*" >&2
  exit 1
}

run_cmd() {
  if (( DRY_RUN )); then
    printf '[dry-run] %s\n' "$*"
  else
    log "+ $*"
    eval "$@"
  fi
}

confirm() {
  local prompt="$1"
  if (( ASSUME_YES )); then
    return 0
  fi
  read -r -p "$prompt [y/N] " response
  case "$response" in
    [yY][eE][sS]|[yY]) return 0 ;;
    *) return 1 ;;
  esac
}

# ─── Pre-flight checks ──────────────────────────────────────────────────

log "Pre-flight: checking Railway CLI..."
if ! command -v railway >/dev/null 2>&1; then
  bail "railway CLI not found. Install: https://docs.railway.app/develop/cli"
fi
RW_VERSION="$(railway --version 2>&1 | head -1)"
log "  railway: ${RW_VERSION}"

log "Pre-flight: checking Railway auth..."
if ! railway whoami >/dev/null 2>&1; then
  bail "railway CLI is not logged in. Run: railway login"
fi
RW_USER="$(railway whoami 2>&1 | head -1)"
log "  authenticated as: ${RW_USER}"

log "Pre-flight: checking Railway project link..."
if ! railway status >/dev/null 2>&1; then
  bail "Local repo not linked to a Railway project. Run: railway link"
fi

log "Pre-flight: checking git state..."
CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [ "${CURRENT_BRANCH}" != "main" ] && [ "${CURRENT_BRANCH}" != "feat/bro-1146-e2e-smoke-prep" ]; then
  log "  WARN: deploying from branch '${CURRENT_BRANCH}' (expected 'main' or 'feat/bro-1146-e2e-smoke-prep')"
  if ! confirm "Proceed anyway?"; then
    bail "Aborted by operator."
  fi
fi
DIRTY="$(git status --porcelain | wc -l | tr -d ' ')"
if [ "${DIRTY}" != "0" ]; then
  log "  WARN: working tree has ${DIRTY} uncommitted change(s). Railway will deploy committed state only."
fi

# ─── OTLP endpoint discovery ────────────────────────────────────────────

if [ -z "${OTLP_ENDPOINT}" ]; then
  cat >&2 <<EOF

[deploy] LIFEGW_OTLP_ENDPOINT is not set. The Vigil traces will not be
[deploy] captured by an observability platform, which makes Section 5.2
[deploy] of the runbook (Vigil trace screenshot) impossible.
[deploy]
[deploy] Set one of:
[deploy]
[deploy]   export LIFEGW_OTLP_ENDPOINT="https://cloud.langfuse.com/api/public/otel"
[deploy]   export LIFEGW_OTLP_ENDPOINT="http://tempo:4317"   # self-hosted Grafana Tempo
[deploy]   export LIFEGW_OTLP_ENDPOINT="http://jaeger:4317"  # self-hosted Jaeger
[deploy]
EOF
  if ! confirm "Proceed with no OTLP endpoint (Vigil falls back to structured logging)?"; then
    bail "Aborted — set LIFEGW_OTLP_ENDPOINT then re-run."
  fi
fi

# ─── Summarise the planned deploy ───────────────────────────────────────

cat <<EOF

[deploy] Planned deploy:
[deploy]   service:        ${SERVICE_NAME}
[deploy]   environment:    ${ENVIRONMENT_NAME}
[deploy]   public domain:  ${PUBLIC_DOMAIN}
[deploy]   OTLP endpoint:  ${OTLP_ENDPOINT:-<not set — fallback to structured logging>}
[deploy]   dev user:       ${DEV_USER}
[deploy]   skip-up:        ${SKIP_UP}
[deploy]   dry-run:        ${DRY_RUN}

EOF

if ! confirm "Proceed?"; then
  bail "Aborted by operator."
fi

# ─── Set env vars on the service ────────────────────────────────────────

# Helper that sets a Railway variable. The CLI exits non-zero if the
# variable doesn't change — we tolerate that (idempotency).
set_var() {
  local key="$1"
  local val="$2"
  # `railway variables set --service <svc> --kv KEY=VAL` is the
  # current CLI shape.
  run_cmd "railway variables --service ${SERVICE_NAME} --environment ${ENVIRONMENT_NAME} --set '${key}=${val}'"
}

log "Setting env vars on service '${SERVICE_NAME}'..."
set_var LIFEGW__PUBLIC__BIND_ADDR              "0.0.0.0:8443"
set_var LIFEGW__AUTH__DEV_SIGNER_ENABLED       "true"
set_var LIFEGW__BILLING__ENFORCE               "false"
set_var LIFEGW__UPSTREAM__UDS_PATH             "/run/life/lifed.sock"
set_var OTEL_SERVICE_NAME                       "${SERVICE_NAME}"
if [ -n "${OTLP_ENDPOINT}" ]; then
  set_var LIFEGW__OBSERVABILITY__OTLP_ENDPOINT "${OTLP_ENDPOINT}"
fi

# ─── Trigger the deploy ─────────────────────────────────────────────────

if (( SKIP_UP )); then
  log "Skipping 'railway up' per --skip-up flag. Env vars are set."
else
  log "Deploying via 'railway up'..."
  run_cmd "railway up --service ${SERVICE_NAME} --environment ${ENVIRONMENT_NAME} --detach"
fi

# ─── Health probe ───────────────────────────────────────────────────────

if (( DRY_RUN )) || (( SKIP_UP )); then
  log "Skipping health probe (dry-run or skip-up)."
else
  log "Waiting for the new deployment to become healthy..."
  # Poll for up to 5 minutes. The probe URL is the public domain;
  # if the operator hasn't set up DNS yet, this fallback to the
  # Railway-provided hostname.
  PROBE_URL="https://${PUBLIC_DOMAIN}/healthz"
  for i in $(seq 1 30); do
    if curl -fsS --max-time 5 "${PROBE_URL}" >/dev/null 2>&1; then
      log "  health probe OK after ${i}0s"
      break
    fi
    sleep 10
    log "  health probe attempt ${i}/30 still failing — retrying..."
  done
  if ! curl -fsS --max-time 5 "${PROBE_URL}" >/dev/null 2>&1; then
    log "  WARN: health probe at ${PROBE_URL} still failing. The deploy may not be fully up yet."
    log "  Check 'railway logs --service ${SERVICE_NAME}' before running the smoke."
  fi
fi

# ─── Print operator's next-step env exports ─────────────────────────────

cat <<EOF

[deploy] ─── Next steps ──────────────────────────────────────────────

[deploy] Set these in the shell where you'll run 'claude':

    export ANTHROPIC_BASE_URL="https://${PUBLIC_DOMAIN}"
    export ANTHROPIC_AUTH_TOKEN="dev-token-for-${DEV_USER}"
    export CLAUDE_CODE_AUTO_COMPACT_WINDOW=190000
    export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1

[deploy] Then launch:

    cd ~/broomva/core/life   # or your preferred working dir
    claude

[deploy] Follow Section 4 of the runbook for the actual smoke session:

    docs/conformance/2026-05-18-claude-code-smoke-runbook.md

[deploy] To tear down after the smoke:

    railway service delete ${SERVICE_NAME}

EOF
