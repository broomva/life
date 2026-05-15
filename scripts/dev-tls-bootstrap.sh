#!/usr/bin/env bash
# Generate a proper local-CA + SAN-bearing leaf cert for lifegw's
# `dev-tls/` directory, so strict-verifying TLS clients (Lumen,
# browsers, curl-without-`-k`, future SDKs) accept HTTPS connections
# to https://127.0.0.1:8443 without per-client bypasses.
#
# Why this exists:
#   The previous `dev-tls/cert.pem` was a self-signed leaf with CN=localhost
#   and no SAN extension. Modern strict TLS verifiers (RFC 2818 / RFC 6125
#   + most browser stacks + reqwest with rustls) reject CN-only certs —
#   they require a SubjectAltName entry naming the host. macOS Security
#   framework rejects with errSecNotTrusted (-67843). Lumen's HTTP client
#   has no `danger_accept_invalid_certs` escape hatch, so Layer 2-GUI
#   couldn't reach lifegw until this script's output landed in dev-tls/.
#
# What this generates (idempotent — safe to re-run):
#   dev-tls/dev-ca.pem        Local CA cert (self-signed, 10-year validity)
#   dev-tls/dev-ca-key.pem    Local CA key (RSA 4096, kept in repo as dev fixture)
#   dev-tls/cert.pem          Server leaf signed by the CA, SAN = DNS:localhost,
#                             DNS:lumen.local, IP:127.0.0.1, IP:::1
#                             (overwrites the existing self-signed leaf)
#   dev-tls/key.pem           Server leaf key (RSA 2048, dev fixture)
#
# After running this script, the user must do ONE more step (sudo):
#
#   sudo security add-trusted-cert -d -r trustRoot \
#     -k /Library/Keychains/System.keychain \
#     crates/life-runtime/lifegw/dev-tls/dev-ca.pem
#
# This adds the LOCAL CA to the system trust store. After that, every
# strict TLS client on this machine trusts certs signed by this CA —
# permanently, until you remove it via:
#
#   sudo security delete-certificate -c "Lumen Dev CA" \
#     /Library/Keychains/System.keychain
#
# Threat surface: the CA private key lives in this repo. Anyone with
# read access to the repo + your machine could forge a cert that your
# machine trusts. This is a dev-only key, never to be deployed; matches
# the threat model of the previous self-signed leaf key already in the
# repo at dev-tls/key.pem.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TLS_DIR="$REPO_ROOT/crates/life-runtime/lifegw/dev-tls"

CA_CRT="$TLS_DIR/dev-ca.pem"
CA_KEY="$TLS_DIR/dev-ca-key.pem"
SRV_CRT="$TLS_DIR/cert.pem"
SRV_KEY="$TLS_DIR/key.pem"

C_INFO=$'\e[1;36m'
C_OK=$'\e[1;32m'
C_DIM=$'\e[2m'
C_RST=$'\e[0m'

info() { printf "${C_INFO}[dev-tls]${C_RST} %s\n" "$*"; }
ok()   { printf "${C_OK}[dev-tls]${C_RST} %s\n" "$*"; }
dim()  { printf "${C_DIM}  %s${C_RST}\n" "$*"; }

mkdir -p "$TLS_DIR"

# ── 1. Local CA ────────────────────────────────────────────────────────────
if [ -f "$CA_KEY" ] && [ -f "$CA_CRT" ]; then
  info "Reusing existing local CA at $CA_CRT"
  dim "$(openssl x509 -in "$CA_CRT" -noout -subject -enddate 2>&1)"
else
  info "Generating fresh local CA (RSA 4096, 10-year validity)"
  openssl genrsa -out "$CA_KEY" 4096 2>/dev/null
  chmod 600 "$CA_KEY"
  openssl req -x509 -new -nodes -key "$CA_KEY" \
    -sha256 -days 3650 \
    -subj "/CN=Lumen Dev CA/O=Broomva/OU=Lumen Phase α" \
    -out "$CA_CRT"
  ok "Local CA: $CA_CRT"
fi

# ── 2. Server leaf cert ────────────────────────────────────────────────────
# Always regenerate the leaf (cheap; ensures SAN is current).
info "Generating server leaf cert (RSA 2048, SAN for localhost/127.0.0.1/::1)"

# OpenSSL 3 lets us pass extensions via `-addext` directly to `req`,
# avoiding the need for an on-disk extfile.
TMP_CSR="$(mktemp -t lumen-csr.XXXXXX)"
trap 'rm -f "$TMP_CSR"' EXIT

openssl genrsa -out "$SRV_KEY" 2048 2>/dev/null
chmod 600 "$SRV_KEY"

openssl req -new -key "$SRV_KEY" -out "$TMP_CSR" \
  -subj "/CN=localhost/O=Broomva/OU=Lumen Phase α"

# Sign with our CA, adding SAN + serverAuth EKU + sensible KU.
# The `-extfile -` trick lets us inline the extensions without a temp file.
openssl x509 -req -in "$TMP_CSR" \
  -CA "$CA_CRT" -CAkey "$CA_KEY" -CAcreateserial \
  -days 825 -sha256 \
  -extfile <(cat <<'EXT'
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = DNS:localhost, DNS:lumen.local, IP:127.0.0.1, IP:0:0:0:0:0:0:0:1
EXT
) \
  -out "$SRV_CRT"

ok "Server leaf: $SRV_CRT"
dim "$(openssl x509 -in "$SRV_CRT" -noout -subject -issuer -enddate 2>&1)"
dim "SAN: $(openssl x509 -in "$SRV_CRT" -noout -ext subjectAltName 2>&1 | tail -1)"

# ── 3. Self-check: leaf chains to CA ───────────────────────────────────────
if openssl verify -CAfile "$CA_CRT" "$SRV_CRT" >/dev/null 2>&1; then
  ok "leaf chains cleanly to CA (openssl verify passed)"
else
  echo "ERROR: leaf does not verify against CA" >&2
  exit 1
fi

# ── 4. Trust hint ──────────────────────────────────────────────────────────
echo
info "Next step (one-time, requires sudo):"
echo
echo "  sudo security add-trusted-cert -d -r trustRoot \\"
echo "    -k /Library/Keychains/System.keychain \\"
echo "    $CA_CRT"
echo
dim "After running that, every strict TLS client on this machine trusts certs"
dim "signed by this dev CA — including Lumen. To revoke, run:"
dim "  sudo security delete-certificate -c \"Lumen Dev CA\" /Library/Keychains/System.keychain"
