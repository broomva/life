#!/usr/bin/env bash
# lifegw-stack entrypoint — fan out lifed + lifegw + caddy in one container.
#
# Stage-2 ordering (May 2026 — addresses the lifegw/lifed boot-race
# described in `HANDOFF.md` §6/§7):
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
#   3. Start `lifed`. Its `JwksCache` is lazy + file-backed (Stage 2
#      change in `lifed::auth::jwks`): the first `validate()` call
#      reads `/run/life/lifegw-jwks.json`, and subsequent calls watch
#      mtime so a rotation is picked up without coordination.
#   4. Start `lifegw` with `kms_provider = "static_pem"` reading the
#      env-bound key. lifegw publishes its JWKS atomically to
#      `/run/life/lifegw-jwks.json`; lifed picks it up on first verify.
#   5. Caddy in foreground as PID 1 (via tini).

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

# ── 3. Start lifed ──────────────────────────────────────────────────────────
echo "[entrypoint] starting lifed (mock-fallback=${LIFED_ALLOW_MOCK_FALLBACK:-false})"
runuser --preserve-environment -u life -g life-runtime -- \
  /usr/local/bin/lifed daemon \
    --config "${LIFED_CONFIG:-/etc/lifed/config.toml}" \
  &
LIFED_PID=$!

echo "[entrypoint] waiting for lifed UDS at ${LIFE_RUNTIME_DIR}/life.sock"
for i in $(seq 1 60); do
  if [[ -S "${LIFE_RUNTIME_DIR}/life.sock" ]]; then
    echo "[entrypoint] lifed UDS ready (after ${i} half-seconds)"
    break
  fi
  if ! kill -0 "${LIFED_PID}" 2>/dev/null; then
    echo "[entrypoint] FATAL: lifed exited before binding UDS" >&2
    wait "${LIFED_PID}" || true
    exit 1
  fi
  sleep 0.5
done
if [[ ! -S "${LIFE_RUNTIME_DIR}/life.sock" ]]; then
  echo "[entrypoint] FATAL: lifed did not bind UDS within 30s" >&2
  exit 1
fi

# ── 4. Start lifegw ─────────────────────────────────────────────────────────
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

# ── 5. Caddy as PID 1's foreground process ─────────────────────────────────
shutdown() {
  echo "[entrypoint] SIGTERM received — draining"
  if kill -0 "${LIFEGW_PID}" 2>/dev/null; then kill -TERM "${LIFEGW_PID}" || true; fi
  if kill -0 "${LIFED_PID}"  2>/dev/null; then kill -TERM "${LIFED_PID}"  || true; fi
}
trap shutdown TERM INT

echo "[entrypoint] starting caddy on :${PORT}"
exec caddy run --config /etc/caddy/Caddyfile --adapter caddyfile
