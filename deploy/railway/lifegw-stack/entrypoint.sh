#!/usr/bin/env bash
# lifegw-stack entrypoint — fan out arcan + lifed + lifegw + caddy in
# one container.
#
# Stage-2 ordering (May 2026 — addresses the lifegw/lifed boot-race
# described in `HANDOFF.md` §6/§7), extended by Stage 5 (June 2026 —
# real arcan substrate inside the stack):
#
#   1. Self-signed TLS cert for the lifegw → Caddy hop. Regenerated each
#      boot — Caddy proxies upstream with `tls_insecure_skip_verify` so
#      the cert chain is irrelevant.
#   2. **Persist a Tier-2 signing keypair** (PKCS#8 PEM). Stored under
#      `LIFE_STATE_DIR` (`/var/life-state` by default — mount a Railway
#      volume there to survive image redeploys; otherwise the key
#      survives container restarts within the same image). Generated
#      once via `openssl genpkey`; reused on every subsequent boot.
#      Operator can also pre-supply the key via the
#      `LIFEGW_TIER2_SIGNING_KEY_PEM` env (e.g. injected by Railway
#      secrets) — entrypoint skips generation in that case.
#   3. Start `arcan serve --uds-socket /run/life/arcan.sock` and probe
#      until it accepts UDS connections. MUST precede lifed: lifed's
#      per-substrate bootstrap samples socket presence once at boot —
#      arcan socket present ⇒ real arcan substrate; lago/haima/anima
#      absent ⇒ in-process mocks (LIFED_ALLOW_MOCK_FALLBACK=true).
#   4. Start `lifed`. Its `JwksCache` is lazy + file-backed (Stage 2
#      change in `lifed::auth::jwks`): the first `validate()` call
#      reads `/run/life/lifegw-jwks.json`, and subsequent calls watch
#      mtime so a rotation is picked up without coordination.
#   5. Start `lifegw` with `kms_provider = "static_pem"` reading the
#      env-bound key. lifegw publishes its JWKS atomically to
#      `/run/life/lifegw-jwks.json`; lifed picks it up on first verify.
#   6. Caddy in foreground as PID 1 (via tini).

set -euo pipefail

# ── 0. Sanity ───────────────────────────────────────────────────────────────
PORT="${PORT:-8080}"
LIFE_RUNTIME_DIR="${LIFE_RUNTIME_DIR:-/run/life}"
TLS_DIR="${TLS_DIR:-/etc/lifegw/tls}"
# Persistent path for the Tier-2 keypair. If a Railway volume is mounted
# at /var/life-state, the key survives container restarts; otherwise it
# lives only for the lifetime of the container — which is still enough
# because `lifed`'s lazy JwksCache picks up the lifegw publish on the
# first verify within that container's lifetime.
LIFE_STATE_DIR="${LIFE_STATE_DIR:-/var/life-state}"
TIER2_PEM_PATH="${TIER2_PEM_PATH:-${LIFE_STATE_DIR}/tier2-signing.pkcs8.pem}"

mkdir -p "${LIFE_RUNTIME_DIR}" "${TLS_DIR}" "${LIFE_STATE_DIR}"
# `life-runtime` group owns /run/life so lifed + lifegw (running as `life`)
# can both bind UDS sockets there with mode 0660.
chown -R life:life-runtime "${LIFE_RUNTIME_DIR}" "${LIFE_STATE_DIR}"
chmod 2775 "${LIFE_RUNTIME_DIR}"
chmod 0750 "${LIFE_STATE_DIR}"

# ── 1. Self-signed cert (regenerated on every boot) ─────────────────────────
if [[ ! -s "${TLS_DIR}/fullchain.pem" || ! -s "${TLS_DIR}/privkey.pem" ]]; then
  echo "[entrypoint] generating self-signed cert in ${TLS_DIR}"
  openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "${TLS_DIR}/privkey.pem" \
    -out    "${TLS_DIR}/fullchain.pem" \
    -days 365 \
    -subj "/CN=lifegw.local" \
    -addext "subjectAltName=DNS:localhost,DNS:lifegw.local,IP:127.0.0.1" \
    >/dev/null 2>&1
  chmod 0640 "${TLS_DIR}/fullchain.pem" "${TLS_DIR}/privkey.pem"
  chown life:life-runtime "${TLS_DIR}/fullchain.pem" "${TLS_DIR}/privkey.pem"
fi

# ── 2. Tier-2 signing key (persistent across container restarts) ────────────
# Three sources, in order of precedence:
#   a) `LIFEGW_TIER2_SIGNING_KEY_PEM` env already set (operator-injected
#      via Railway secrets / external KMS shim) — entrypoint trusts it
#      verbatim and writes it to disk so subsequent reads short-circuit.
#   b) `${TIER2_PEM_PATH}` already exists on disk (typical: Railway
#      volume mounted at /var/life-state — key persists across redeploys).
#   c) Neither — entrypoint generates a fresh PKCS#8 PEM.
if [[ -n "${LIFEGW_TIER2_SIGNING_KEY_PEM:-}" ]]; then
  echo "[entrypoint] using operator-provided Tier-2 PEM from env"
  printf '%s\n' "${LIFEGW_TIER2_SIGNING_KEY_PEM}" >"${TIER2_PEM_PATH}"
  chmod 0600 "${TIER2_PEM_PATH}"
  chown life:life-runtime "${TIER2_PEM_PATH}"
elif [[ ! -s "${TIER2_PEM_PATH}" ]]; then
  echo "[entrypoint] generating Tier-2 signing key at ${TIER2_PEM_PATH}"
  openssl genpkey \
    -algorithm EC \
    -pkeyopt ec_paramgen_curve:P-256 \
    -out "${TIER2_PEM_PATH}" \
    >/dev/null 2>&1
  chmod 0600 "${TIER2_PEM_PATH}"
  chown life:life-runtime "${TIER2_PEM_PATH}"
else
  echo "[entrypoint] reusing existing Tier-2 key at ${TIER2_PEM_PATH}"
fi

# Export for lifegw's `KmsProvider::StaticPem` arm. lifegw reads the env
# at signer-build time (see `lifegw::bootstrap::build_signer`).
LIFEGW_TIER2_SIGNING_KEY_PEM="$(cat "${TIER2_PEM_PATH}")"
export LIFEGW_TIER2_SIGNING_KEY_PEM

# ── 3. Start arcan (real arcan substrate) ───────────────────────────────────
# Stage 5 (June 2026): lifed's bootstrap samples substrate UDS presence
# ONCE at boot (per-substrate selection — see lifed::bootstrap). arcan
# must therefore bind /run/life/arcan.sock and be ACCEPTING connections
# BEFORE lifed starts, or lifed honestly falls back to MockArcan for
# the container's lifetime. lago/haima/anima remain mocked via
# LIFED_ALLOW_MOCK_FALLBACK=true until their daemons ship.
#
# The flag is `--uds-socket` (env ARCAN_UDS_SOCKET) — binds the
# substrate-plane gRPC server (arcan.v1.AgentSubstrate, BRO-1016)
# alongside arcan's HTTP :3000 server (container-internal only).
# Provider preflight: arcan's build_provider runs BEFORE the UDS server
# binds (main.rs), so a fresh container without provider credentials
# would exit at boot and crash-loop the whole gateway. Degrade honestly
# instead: skip arcan so lifed selects MockArcan, and say exactly which
# env unlocks the real substrate.
ARCAN_ENABLED=1
if [[ -z "${ARCAN_PROVIDER:-}" && -z "${ANTHROPIC_API_KEY:-}" ]]; then
  ARCAN_ENABLED=0
  echo "[entrypoint] WARN: skipping arcan substrate — no provider env." >&2
  echo "[entrypoint]   set ANTHROPIC_API_KEY (default provider) or ARCAN_PROVIDER=openai + OPENAI_BASE_URL/OPENAI_API_KEY" >&2
  echo "[entrypoint]   lifed will select MockArcan for this container lifetime." >&2
fi

if [[ "${ARCAN_ENABLED}" == "1" ]]; then
ARCAN_DATA_DIR="${ARCAN_DATA_DIR:-/var/lib/arcan}"
mkdir -p "${ARCAN_DATA_DIR}"
chown -R life:life-runtime "${ARCAN_DATA_DIR}"
echo "[entrypoint] starting arcan substrate (uds=${LIFE_RUNTIME_DIR}/arcan.sock data=${ARCAN_DATA_DIR})"
# `env -u`: the Tier-2 token-minting key is lifegw's secret. arcan
# executes tool calls (incl. shell) for remote chat users — that key
# must never be readable from the agent process environment.
runuser --preserve-environment -u life -g life-runtime -- \
  env -u LIFEGW_TIER2_SIGNING_KEY_PEM \
  /usr/local/bin/arcan serve \
    --uds-socket "${LIFE_RUNTIME_DIR}/arcan.sock" \
    --data-dir "${ARCAN_DATA_DIR}" \
    --agents-dir /opt/life/agents \
  &
ARCAN_PID=$!

# Same probe discipline as the lifed probe below (BRO-1193): `nc -zU`
# proves the listen backlog is up, not merely that bind() created the
# socket file. A half-bound socket here would make lifed dial a dead
# arcan at boot and fail the whole stack.
echo "[entrypoint] waiting for arcan UDS at ${LIFE_RUNTIME_DIR}/arcan.sock"
for i in $(seq 1 60); do
  if [[ -S "${LIFE_RUNTIME_DIR}/arcan.sock" ]] \
     && nc -zU "${LIFE_RUNTIME_DIR}/arcan.sock" 2>/dev/null; then
    echo "[entrypoint] arcan UDS accepting connections (after ${i} half-seconds)"
    break
  fi
  if ! kill -0 "${ARCAN_PID}" 2>/dev/null; then
    echo "[entrypoint] FATAL: arcan exited before binding UDS" >&2
    wait "${ARCAN_PID}" || true
    exit 1
  fi
  sleep 0.5
done
if ! nc -zU "${LIFE_RUNTIME_DIR}/arcan.sock" 2>/dev/null; then
  echo "[entrypoint] FATAL: arcan did not accept UDS connections within 30s" >&2
  exit 1
fi
fi # ARCAN_ENABLED

# ── 4. Start lifed ──────────────────────────────────────────────────────────
# Stage 5: lifed's per-substrate bootstrap sees /run/life/arcan.sock
# (bound above) and dials the REAL arcan substrate; lago/haima/anima
# sockets are absent so they fall back to mocks under
# LIFED_ALLOW_MOCK_FALLBACK=true. Boot log prints the selection, e.g.
# "arcan=real lago=mock haima=mock anima=mock".
# Stage 3b knob retained: `LIFED_ARCAN_BACKEND=vercel_ai_gateway` only
# applies when the arcan socket is ABSENT (the real substrate wins).
echo "[entrypoint] starting lifed (mock-fallback=${LIFED_ALLOW_MOCK_FALLBACK:-false}, arcan-backend=${LIFED_ARCAN_BACKEND:-mock})"
runuser --preserve-environment -u life -g life-runtime -- \
  /usr/local/bin/lifed daemon \
    --config "${LIFED_CONFIG:-/etc/lifed/config.toml}" \
  &
LIFED_PID=$!

# BRO-1193: probe the socket with `nc -zU` instead of only checking
# whether the file is a socket. `[[ -S ... ]]` passes the instant
# lifed `bind()`s the UDS, but `bind()` happens *before* `listen()` +
# `accept()` make the kernel ready to queue connections. lifegw boots
# 1-2ms after the file appears, tries to `connect()`, and gets a
# `transport error` against an unlistening socket. The fix forces an
# actual UDS connect: kernel only succeeds when the listen backlog is
# up. See `lifegw::bootstrap` "Error: upstream: dial uds: transport
# error" → "[entrypoint] FATAL: lifegw exited before binding 127.0.0.1:8443"
# crash loop in production (2026-05-20T04:18:40 onward).
echo "[entrypoint] waiting for lifed UDS at ${LIFE_RUNTIME_DIR}/life.sock"
for i in $(seq 1 60); do
  # Belt-and-suspenders: cheap file-existence check first; only attempt
  # the nc probe once the file is a socket. nc -zU returns 0 only when
  # connect() succeeds — i.e., listen backlog is ready.
  if [[ -S "${LIFE_RUNTIME_DIR}/life.sock" ]] \
     && nc -zU "${LIFE_RUNTIME_DIR}/life.sock" 2>/dev/null; then
    echo "[entrypoint] lifed UDS accepting connections (after ${i} half-seconds)"
    break
  fi
  if ! kill -0 "${LIFED_PID}" 2>/dev/null; then
    echo "[entrypoint] FATAL: lifed exited before binding UDS" >&2
    wait "${LIFED_PID}" || true
    exit 1
  fi
  sleep 0.5
done
if ! nc -zU "${LIFE_RUNTIME_DIR}/life.sock" 2>/dev/null; then
  echo "[entrypoint] FATAL: lifed did not accept UDS connections within 30s" >&2
  exit 1
fi

# ── 5. Start lifegw ─────────────────────────────────────────────────────────
# lifegw publishes /run/life/lifegw-jwks.json atomically (write-tmp +
# rename) inside its bootstrap. lifed's lazy JwksCache reads it on the
# first verify — no coordination needed because the file mtime always
# advances when lifegw rewrites it.
echo "[entrypoint] starting lifegw (kms_provider=static_pem)"
runuser --preserve-environment -u life -g life-runtime -- \
  /usr/local/bin/lifegw daemon \
    --config "${LIFEGW_CONFIG:-/etc/lifegw/config.toml}" \
  &
LIFEGW_PID=$!

echo "[entrypoint] waiting for lifegw on 127.0.0.1:8443"
for i in $(seq 1 60); do
  if (echo > /dev/tcp/127.0.0.1/8443) 2>/dev/null; then
    echo "[entrypoint] lifegw listening (after ${i} half-seconds)"
    break
  fi
  if ! kill -0 "${LIFEGW_PID}" 2>/dev/null; then
    echo "[entrypoint] FATAL: lifegw exited before binding 127.0.0.1:8443" >&2
    wait "${LIFEGW_PID}" || true
    exit 1
  fi
  sleep 0.5
done

# ── 6. Caddy as PID 1's foreground process ─────────────────────────────────
shutdown() {
  echo "[entrypoint] SIGTERM received — draining"
  if kill -0 "${LIFEGW_PID}" 2>/dev/null; then kill -TERM "${LIFEGW_PID}" || true; fi
  if kill -0 "${LIFED_PID}"  2>/dev/null; then kill -TERM "${LIFED_PID}"  || true; fi
  if [[ -n "${ARCAN_PID:-}" ]] && kill -0 "${ARCAN_PID}" 2>/dev/null; then kill -TERM "${ARCAN_PID}" || true; fi
}
trap shutdown TERM INT

echo "[entrypoint] starting caddy on :${PORT}"
exec caddy run --config /etc/caddy/Caddyfile --adapter caddyfile
