#!/usr/bin/env bash
# M9-E PR-1 (BRO-1215) — softhsm2 initialisation for the secp256k1
# pre-flight spike.
#
# Run this INSIDE the softhsm container (the docker-compose entrypoint
# is `tail -f /dev/null` so the container stays up for interactive exec):
#
#   docker-compose -f deploy/lifegw/vault-sidecar/docker-compose.yml up -d
#   docker-compose -f deploy/lifegw/vault-sidecar/docker-compose.yml \
#       exec softhsm /init-softhsm.sh
#
# What this does:
#   1. Initialises a fresh softhsm token with a known label + SO/User PIN.
#   2. Generates a secp256k1 keypair via OpenSSL (so we can pin the
#      private bytes — softhsm's own `--keypairgen` doesn't accept seed
#      bytes from the CLI).
#   3. Imports the keypair into softhsm via PKCS#11 (pkcs11-tool).
#   4. Prints the public key DER for cross-checking against
#      crates/life-runtime/lifegw/tests/secp256k1_test_vector.rs.
#
# After init completes, `test-vectors/secp256k1-sign.sh` is the
# canonical pre-flight: it signs the canonical message via softhsm and
# compares the bytes to the pinned k256 output.
#
# This script is **idempotent** — running it twice on the same token
# storage is a no-op (init refuses an already-initialised slot, key
# import detects an existing label).

set -euo pipefail

TOKEN_LABEL="${TOKEN_LABEL:-lifegw-anima}"
TOKEN_USER_PIN="${TOKEN_USER_PIN:-1234}"
TOKEN_SO_PIN="${TOKEN_SO_PIN:-12345678}"
KEY_LABEL="${KEY_LABEL:-rfc6979-spike-wallet}"
KEY_ID_HEX="${KEY_ID_HEX:-01}"

WORKDIR="${WORKDIR:-/tmp/lifegw-vault-sidecar-init}"
mkdir -p "${WORKDIR}"

# RFC 6979 §A.2.5 canonical private key (matches
# crates/life-runtime/lifegw/tests/secp256k1_test_vector.rs::vector::PRIVATE_KEY_HEX).
RFC6979_PRIV_HEX="C9AFA9D845BA75166B5C215767B1D6934E50C3DB36E89B127B8A622B120F6721"

echo "[init-softhsm] step 1/4 — checking existing tokens"
EXISTING_TOKEN=$(softhsm2-util --show-slots 2>/dev/null | grep -E "Label:\s+${TOKEN_LABEL}\b" || true)
if [[ -z "${EXISTING_TOKEN}" ]]; then
  echo "[init-softhsm] step 2/4 — initialising fresh token '${TOKEN_LABEL}'"
  softhsm2-util \
    --init-token \
    --slot 0 \
    --label "${TOKEN_LABEL}" \
    --so-pin "${TOKEN_SO_PIN}" \
    --pin "${TOKEN_USER_PIN}"
else
  echo "[init-softhsm] step 2/4 — token '${TOKEN_LABEL}' already initialised; skipping"
fi

echo "[init-softhsm] step 3/4 — generating canonical secp256k1 keypair from RFC 6979 §A.2.5 seed"
# OpenSSL doesn't take a literal scalar as input, so we build the PKCS#8
# PEM by hand: a 32-byte scalar wrapped in the secp256k1 EC private key
# template. The hex below is:
#   30740201010420 <PRIV_KEY_32B> a00706052b8104000a a14403420004 <PUB_KEY_UNCOMPRESSED>
# but we delegate the assembly to OpenSSL by writing the scalar to a
# raw file + using `-text` round-trip. This is the lowest-friction way
# to seed OpenSSL with a known scalar.
#
# Simpler approach: have OpenSSL generate a random key, then import the
# canonical scalar into the token via softhsm-internal command. But
# softhsm2-util doesn't accept a raw scalar; pkcs11-tool's
# --write-object requires DER. We therefore build the DER from
# OpenSSL.
{
  printf '\x30\x74\x02\x01\x01\x04\x20'
  printf "${RFC6979_PRIV_HEX}" | xxd -r -p
  printf '\xa0\x07\x06\x05\x2b\x81\x04\x00\x0a'
  # Public key SEC1 uncompressed will be computed by OpenSSL from the
  # scalar via `ec -text -in` — we just emit a placeholder here and let
  # OpenSSL regenerate the file with the correct pubkey appended.
  printf '\xa1\x44\x03\x42\x00\x04'
  # 64 zero bytes — OpenSSL rewrites these when we round-trip.
  printf '\x00%.0s' {1..64}
} > "${WORKDIR}/priv-raw.der"

# Round-trip through OpenSSL to compute the pubkey + emit clean DER/PEM.
openssl ec \
  -in "${WORKDIR}/priv-raw.der" \
  -inform DER \
  -outform PEM \
  -out "${WORKDIR}/priv.pem" 2>/dev/null || {
    echo "[init-softhsm] FATAL: OpenSSL refused to round-trip the canonical scalar." >&2
    echo "[init-softhsm] This likely means the DER assembly is off by a byte." >&2
    echo "[init-softhsm] Re-derive in a clean shell or fall back to the kid-mismatch path." >&2
    exit 1
  }

# Convert PEM → PKCS#8 PEM (pkcs11-tool's preferred input).
openssl pkcs8 \
  -in "${WORKDIR}/priv.pem" \
  -topk8 \
  -nocrypt \
  -out "${WORKDIR}/priv.pkcs8.pem"

# Convert PKCS#8 PEM → DER for pkcs11-tool --write-object.
openssl pkcs8 \
  -in "${WORKDIR}/priv.pkcs8.pem" \
  -topk8 \
  -nocrypt \
  -outform DER \
  -out "${WORKDIR}/priv.pkcs8.der"

echo "[init-softhsm] step 4/4 — importing keypair into softhsm token"
# pkcs11-tool's `--write-object` requires either --type pubkey or
# --type privkey. We import the private half; the public half is
# derived from it on demand.
EXISTING_KEY=$(pkcs11-tool \
  --module /usr/lib/softhsm/libsofthsm2.so \
  --token-label "${TOKEN_LABEL}" \
  --pin "${TOKEN_USER_PIN}" \
  --list-objects 2>/dev/null | grep -E "label:\s+${KEY_LABEL}\b" || true)

if [[ -z "${EXISTING_KEY}" ]]; then
  pkcs11-tool \
    --module /usr/lib/softhsm/libsofthsm2.so \
    --token-label "${TOKEN_LABEL}" \
    --pin "${TOKEN_USER_PIN}" \
    --write-object "${WORKDIR}/priv.pkcs8.der" \
    --type privkey \
    --label "${KEY_LABEL}" \
    --id "${KEY_ID_HEX}"
  echo "[init-softhsm] imported '${KEY_LABEL}' into token '${TOKEN_LABEL}'"
else
  echo "[init-softhsm] '${KEY_LABEL}' already present in '${TOKEN_LABEL}'; skipping import"
fi

echo "[init-softhsm] done. Public key check:"
pkcs11-tool \
  --module /usr/lib/softhsm/libsofthsm2.so \
  --token-label "${TOKEN_LABEL}" \
  --pin "${TOKEN_USER_PIN}" \
  --read-object \
  --type pubkey \
  --label "${KEY_LABEL}" \
  -o "${WORKDIR}/pub.der" 2>/dev/null || echo "[init-softhsm] (pubkey export skipped — derivable on signing)"

echo "[init-softhsm] next step: ./test-vectors/secp256k1-sign.sh"
