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
#   - the target Railway SERVICE exists ("lifegw-spec-j" by default —
#     create it via the Railway dashboard or `railway add --service
#     lifegw-spec-j` before running this script; the script's
#     service-existence pre-flight will bail with these instructions
#     if it doesn't find the service)
#
# Post-conditions:
#   - The baked `deploy/railway/lifegw-stack/lifegw.toml` is edited
#     in-place to enable the dev signer (`dev_signer_enabled = true`)
#     BEFORE `railway up`, because lifegw reads only TOML (not env
#     vars) and the production posture in the baked TOML is
#     `dev_signer_enabled = false`. The script writes a `.bak` backup
#     and warns the operator to revert before committing.
#   - `RAILWAY_DOCKERFILE_PATH=deploy/railway/lifegw-stack/Dockerfile`
#     is set on the service so Railway uses the multi-process
#     Dockerfile instead of nixpacks-autodetecting.
#   - `OTEL_SERVICE_NAME` + `LIFEGW_OTLP_ENDPOINT` env vars are set
#     (these are read by the entrypoint / vigil layer, not by
#     lifegw's TOML loader).
#   - lifegw is deployed to the staging environment (unless --skip-up).
#   - The script prints the public URL + ANTHROPIC_AUTH_TOKEN the
#     operator copy-pastes into the Claude Code shell.
#
# Honesty note: the only knobs lifegw reads at runtime are in the TOML
# pointed at by `LIFEGW_CONFIG` (set by the Dockerfile to
# `/etc/lifegw/config.toml`). Env-var-style overrides like
# `LIFEGW__AUTH__DEV_SIGNER_ENABLED` are NOT consumed by lifegw — they
# would be silent no-ops. That's why this script edits the TOML in
# place rather than setting env vars. See
# `crates/life-runtime/lifegw/src/main.rs:27` + `src/config.rs:626`
# for the actual loader surface.

set -euo pipefail

# ─── Configuration ──────────────────────────────────────────────────────

SERVICE_NAME="${LIFEGW_SERVICE_NAME:-lifegw-spec-j}"
ENVIRONMENT_NAME="${LIFEGW_ENVIRONMENT:-staging}"
PUBLIC_DOMAIN="${LIFEGW_PUBLIC_DOMAIN:-lifegw-spec-j.broomva.dev}"
OTLP_ENDPOINT="${LIFEGW_OTLP_ENDPOINT:-}"  # operator overrides
DEV_USER="${LIFEGW_DEV_USER:-broomva}"

# Repo-relative path to the baked lifegw config the Railway image
# uses. The deploy script edits this in place (with a `.bak` backup)
# to flip `dev_signer_enabled = false → true` before `railway up`,
# because lifegw doesn't read env vars.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." &>/dev/null && pwd)"
LIFEGW_BAKED_TOML="${REPO_ROOT}/deploy/railway/lifegw-stack/lifegw.toml"
DOCKERFILE_RELATIVE_PATH="deploy/railway/lifegw-stack/Dockerfile"

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

log "Pre-flight: confirming target service '${SERVICE_NAME}' exists..."
# `railway service link <name>` exits non-zero if the service does
# not exist in the linked project. We do the existence check this way
# because the CLI does not expose a dedicated `service ls`/`service
# exists` shape in 4.x. Running `service link` is harmless: it just
# (re-)pins the linked service to the provided name. If the operator
# wanted a different default service linked locally they can re-run
# `railway service link <other>` after this script finishes.
if ! railway service link "${SERVICE_NAME}" >/dev/null 2>&1; then
  cat >&2 <<EOF
[deploy] ERROR: service '${SERVICE_NAME}' does not exist in the linked Railway project.
[deploy]
[deploy]        Create it first (one of):
[deploy]
[deploy]          # CLI:
[deploy]          railway add --service ${SERVICE_NAME}
[deploy]
[deploy]          # Dashboard:
[deploy]          open https://railway.app and add an empty service named '${SERVICE_NAME}'.
[deploy]
[deploy]        Then re-run this script.
EOF
  exit 1
fi
log "  service '${SERVICE_NAME}' linked successfully"

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

log "Pre-flight: confirming baked lifegw config exists..."
if [ ! -f "${LIFEGW_BAKED_TOML}" ]; then
  bail "Expected baked config at '${LIFEGW_BAKED_TOML}' — repo layout has drifted from this script's assumptions."
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

# Helper that prints a redacted view of an OTLP endpoint for
# operator logs. URLs of the form `https://<user>:<pass>@host/...`
# get the userinfo segment redacted; bare URLs pass through
# verbatim. Auth-via-header endpoints (the recommended Langfuse
# shape) don't need this — the bearer is in a separate env var.
redact_otlp() {
  local url="$1"
  if [[ "${url}" =~ ^([a-z]+://)([^:@/]+):([^@/]+)@(.+)$ ]]; then
    printf '%s%s:%s@%s' \
      "${BASH_REMATCH[1]}" \
      "${BASH_REMATCH[2]}" \
      "<redacted>" \
      "${BASH_REMATCH[4]}"
  else
    printf '%s' "${url}"
  fi
}

# ─── Summarise the planned deploy ───────────────────────────────────────

cat <<EOF

[deploy] Planned deploy:
[deploy]   service:        ${SERVICE_NAME}
[deploy]   environment:    ${ENVIRONMENT_NAME}
[deploy]   public domain:  ${PUBLIC_DOMAIN}
[deploy]   OTLP endpoint:  ${OTLP_ENDPOINT:+$(redact_otlp "${OTLP_ENDPOINT}")}${OTLP_ENDPOINT:-<not set — fallback to structured logging>}
[deploy]   dev user:       ${DEV_USER}
[deploy]   baked TOML:     ${LIFEGW_BAKED_TOML#${REPO_ROOT}/}
[deploy]   dockerfile:     ${DOCKERFILE_RELATIVE_PATH}
[deploy]   skip-up:        ${SKIP_UP}
[deploy]   dry-run:        ${DRY_RUN}

[deploy] WARNING — TOML mutation:
[deploy]   This script edits ${LIFEGW_BAKED_TOML#${REPO_ROOT}/}
[deploy]   in place to set 'dev_signer_enabled = true' before deploy.
[deploy]   A .bak backup is written. Revert before committing the repo.

EOF

if ! confirm "Proceed?"; then
  bail "Aborted by operator."
fi

# ─── Edit baked TOML to enable dev signer (C1 fix) ─────────────────────
#
# lifegw reads ONLY the TOML file pointed at by `LIFEGW_CONFIG`. The
# baked TOML at `deploy/railway/lifegw-stack/lifegw.toml` has
# `dev_signer_enabled = false` (Stage 4 production posture). For the
# Phase 1 smoke we need it `true` so `Bearer dev-token-for-<user>` is
# accepted. We patch the TOML in place + write a `.bak`, then warn
# the operator to revert before committing.
#
# This is intentionally simpler than wiring env-var overlays into
# lifegw's config loader. The smoke is a one-off ephemeral staging
# deploy; the TOML edit is a deploy-time-only mutation reverted
# after smoke teardown.
log "Patching baked TOML to enable dev signer for staging..."
if (( DRY_RUN )); then
  log "[dry-run] would patch ${LIFEGW_BAKED_TOML#${REPO_ROOT}/}:"
  log "[dry-run]   dev_signer_enabled = false → true"
  log "[dry-run] would write ${LIFEGW_BAKED_TOML#${REPO_ROOT}/}.bak"
else
  if grep -q '^dev_signer_enabled[[:space:]]*=[[:space:]]*true' "${LIFEGW_BAKED_TOML}"; then
    log "  TOML already has dev_signer_enabled = true (idempotent: no edit needed)"
  else
    cp -- "${LIFEGW_BAKED_TOML}" "${LIFEGW_BAKED_TOML}.bak"
    # macOS sed needs `-i ''`; GNU sed accepts `-i` without arg. Use a
    # portable two-step: write to a tempfile then mv.
    tmp="$(mktemp)"
    sed 's/^dev_signer_enabled[[:space:]]*=[[:space:]]*false/dev_signer_enabled  = true/' \
      "${LIFEGW_BAKED_TOML}" >"${tmp}"
    if ! grep -q '^dev_signer_enabled[[:space:]]*=[[:space:]]*true' "${tmp}"; then
      rm -f "${tmp}"
      bail "TOML edit did not produce 'dev_signer_enabled = true' — pattern mismatch. Inspect ${LIFEGW_BAKED_TOML} manually."
    fi
    mv -- "${tmp}" "${LIFEGW_BAKED_TOML}"
    log "  patched: dev_signer_enabled = true  (backup at ${LIFEGW_BAKED_TOML#${REPO_ROOT}/}.bak)"
  fi
fi

# ─── Set env vars on the service ────────────────────────────────────────
#
# Honesty: lifegw does NOT read most of these. The ones that ARE
# consumed:
#   - RAILWAY_DOCKERFILE_PATH — read by Railway's build system to
#     pick the multi-process Dockerfile. Without this, Railway
#     nixpacks-autodetects against the monorepo root and fails.
#   - LIFEGW_OTLP_ENDPOINT  — read by `life_vigil::init_telemetry`
#     via env, not lifegw config.
#   - OTEL_SERVICE_NAME     — OpenTelemetry SDK convention.
#
# The previous list of `LIFEGW__*` env vars was wrong; lifegw's
# TOML-only loader silently ignores them. They've been removed.
# `dev_signer_enabled` + `billing.enforce` flip via the TOML edit
# above (C1 fix), not env.

# Helper that sets a Railway variable. The CLI exits non-zero if the
# variable doesn't change — we tolerate that (idempotency).
set_var() {
  local key="$1"
  local val="$2"
  # `railway variables set --service <svc> --kv KEY=VAL` is the
  # current CLI shape.
  run_cmd "railway variables --service ${SERVICE_NAME} --environment ${ENVIRONMENT_NAME} --set '${key}=${val}'"
}

# Helper for vars whose values may carry secrets in URL form — same
# action, but the printed `+ railway ...` log line is redacted.
set_var_redacted() {
  local key="$1"
  local val="$2"
  local redacted
  redacted="$(redact_otlp "${val}")"
  if (( DRY_RUN )); then
    printf '[dry-run] railway variables --service %s --environment %s --set %s=%s\n' \
      "${SERVICE_NAME}" "${ENVIRONMENT_NAME}" "${key}" "${redacted}"
  else
    log "+ railway variables --service ${SERVICE_NAME} --environment ${ENVIRONMENT_NAME} --set '${key}=${redacted}'"
    # Pass the real value to the CLI but never echo it.
    railway variables --service "${SERVICE_NAME}" --environment "${ENVIRONMENT_NAME}" \
      --set "${key}=${val}"
  fi
}

log "Setting env vars on service '${SERVICE_NAME}'..."
# Railway-side build picker (C3 fix): without this, Railway
# nixpacks-autodetects against the monorepo root and the build fails
# without using our multi-process Dockerfile.
set_var RAILWAY_DOCKERFILE_PATH                 "${DOCKERFILE_RELATIVE_PATH}"
# OpenTelemetry env vars (consumed by life_vigil at runtime).
set_var OTEL_SERVICE_NAME                       "${SERVICE_NAME}"
if [ -n "${OTLP_ENDPOINT}" ]; then
  # Use the redacted setter for the OTLP endpoint — operators
  # sometimes pass `https://<key>:<secret>@host/...` URL-style auth.
  set_var_redacted LIFEGW_OTLP_ENDPOINT          "${OTLP_ENDPOINT}"
fi
# `LIFED_ALLOW_MOCK_FALLBACK` is already baked into the Dockerfile
# (default `true` for the lifegw-stack image), but we set it
# explicitly on the service so the operator can flip it via Railway
# Dashboard without a re-deploy if they migrate to Option B.
set_var LIFED_ALLOW_MOCK_FALLBACK               "true"

# ─── Trigger the deploy ─────────────────────────────────────────────────

if (( SKIP_UP )); then
  log "Skipping 'railway up' per --skip-up flag. Env vars are set + TOML patched."
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
  # if the operator hasn't set up DNS yet, replace PUBLIC_DOMAIN
  # with the Railway-provided hostname before re-running.
  PROBE_URL="https://${PUBLIC_DOMAIN}/healthz"
  PROBE_OK=0
  for i in $(seq 1 30); do
    if curl -fsS --max-time 5 "${PROBE_URL}" >/dev/null 2>&1; then
      log "  health probe OK after ${i}0s"
      PROBE_OK=1
      break
    fi
    sleep 10
    log "  health probe attempt ${i}/30 still failing — retrying..."
  done
  if (( ! PROBE_OK )); then
    cat >&2 <<EOF
[deploy] FAIL: health probe at ${PROBE_URL} never returned 200 within 5 minutes.
[deploy]
[deploy]       The deploy is NOT ready. The runbook smoke will not
[deploy]       succeed against this state.
[deploy]
[deploy]       Diagnose:
[deploy]         railway logs --service ${SERVICE_NAME}
[deploy]         railway status --service ${SERVICE_NAME}
[deploy]
[deploy]       Common causes:
[deploy]         - DNS for ${PUBLIC_DOMAIN} not yet pointing at Railway
[deploy]           edge (try the Railway-generated *.railway.app URL).
[deploy]         - Build failure (check 'railway logs').
[deploy]         - Service not bound to the multi-process Dockerfile
[deploy]           (verify RAILWAY_DOCKERFILE_PATH is set on the
[deploy]           service via 'railway variables --service ${SERVICE_NAME}').
EOF
    exit 1
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

[deploy] Reminder — TOML mutation:
[deploy]   This script edited ${LIFEGW_BAKED_TOML#${REPO_ROOT}/}
[deploy]   (dev_signer_enabled = false → true).
[deploy]   Backup at ${LIFEGW_BAKED_TOML#${REPO_ROOT}/}.bak.
[deploy]   Restore before committing:
[deploy]
[deploy]     mv ${LIFEGW_BAKED_TOML#${REPO_ROOT}/}.bak \\
[deploy]        ${LIFEGW_BAKED_TOML#${REPO_ROOT}/}

[deploy] To tear down after the smoke (removes the most recent deployment;
[deploy] keeps the service shell so re-deploy is one command):

    railway down --service ${SERVICE_NAME} --environment ${ENVIRONMENT_NAME} --yes

[deploy] To fully remove the SERVICE (CLI does not support this in 4.x —
[deploy] use the Railway Dashboard):

    open https://railway.app   # Project → service '${SERVICE_NAME}' → Settings → Delete

EOF
