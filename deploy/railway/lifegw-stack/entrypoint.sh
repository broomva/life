#!/usr/bin/env bash
# lifegw-stack entrypoint — fan out lifed + lifegw + caddy in one container.
#
# Order matters:
#   1. Generate self-signed cert at /etc/lifegw/tls/{fullchain,privkey}.pem
#      so lifegw passes its TLS-bind step. The cert is regenerated on every
#      container boot — Caddy proxies upstream with `tls_insecure_skip_verify`,
#      so trust-chain validity is irrelevant.
#   2. Start lifed in the background. It binds /run/life/life.sock + the
#      admin socket. Wait for the public socket to appear before starting
#      lifegw (which would otherwise race the UDS connect).
#   3. Start lifegw in the background. It listens on 127.0.0.1:8443.
#   4. Wait for 8443 to accept connections, then exec caddy in foreground.
#      caddy becomes the supervisor — SIGTERM lands on caddy first, which
#      tini propagates to the children.

set -euo pipefail

# ── 0. Sanity ───────────────────────────────────────────────────────────────
PORT="${PORT:-8080}"
LIFE_RUNTIME_DIR="${LIFE_RUNTIME_DIR:-/run/life}"
TLS_DIR="${TLS_DIR:-/etc/lifegw/tls}"

mkdir -p "${LIFE_RUNTIME_DIR}" "${TLS_DIR}"
# `life-runtime` group owns /run/life so lifed + lifegw (running as `life`)
# can both bind UDS sockets there with mode 0660.
chown -R life:life-runtime "${LIFE_RUNTIME_DIR}"
chmod 2775 "${LIFE_RUNTIME_DIR}"

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

# ── 2. Start lifed ──────────────────────────────────────────────────────────
echo "[entrypoint] starting lifed (mock-fallback=${LIFED_ALLOW_MOCK_FALLBACK:-0})"
# Run lifed under the `life` user so its UDS sockets land owned by life:life-runtime.
# `setpriv` would be cleaner but isn't on slim — `runuser` is.
runuser -u life -g life-runtime -- \
  /usr/local/bin/lifed daemon \
    --config "${LIFED_CONFIG:-/etc/lifed/config.toml}" \
  &
LIFED_PID=$!

# Wait for /run/life/life.sock to exist (max 30s — bootstrap reads lago,
# JWKS publish, etc.). Failure here is fatal: caddy + lifegw would never
# come up healthy without lifed.
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

# ── 3. Start lifegw ─────────────────────────────────────────────────────────
echo "[entrypoint] starting lifegw"
runuser -u life -g life-runtime -- \
  /usr/local/bin/lifegw daemon \
    --config "${LIFEGW_CONFIG:-/etc/lifegw/config.toml}" \
  &
LIFEGW_PID=$!

# Wait for 127.0.0.1:8443 to accept TCP. We don't need to handshake TLS;
# a successful connect is enough proof the listener is live.
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

# ── 4. Caddy as PID 1's foreground process ─────────────────────────────────
# `caddy run` is the long-lived foreground process. SIGTERM from Railway
# lands on caddy via tini; we trap it here to fan-out a graceful shutdown
# to the background daemons before exiting.
shutdown() {
  echo "[entrypoint] SIGTERM received — draining"
  if kill -0 "${LIFEGW_PID}" 2>/dev/null; then kill -TERM "${LIFEGW_PID}" || true; fi
  if kill -0 "${LIFED_PID}"  2>/dev/null; then kill -TERM "${LIFED_PID}"  || true; fi
  # Caddy itself is signaled by tini; nothing else to do.
}
trap shutdown TERM INT

echo "[entrypoint] starting caddy on :${PORT}"
exec caddy run --config /etc/caddy/Caddyfile --adapter caddyfile
