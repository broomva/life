#!/usr/bin/env bash
# M9-E PR-1 (BRO-1215) — softhsm secp256k1 deterministic-ECDSA pre-flight.
#
# Signs the canonical SHA-256("sample") digest via softhsm's PKCS#11
# module and compares the resulting r||s to the pinned bytes from
# `crates/life-runtime/lifegw/tests/secp256k1_test_vector.rs`.
#
# Exit code:
#   0 → softhsm produced the expected deterministic signature; signing
#       path is sound and the M9-E stopgap is approved.
#   1 → mismatch. STOP. Do NOT proceed with the production deploy.
#       Investigate: softhsm version, OpenSSL version, PKCS#11 module
#       path. The most common cause is a softhsm build without the
#       secp256k1 OID compiled in.
#
# Run inside the softhsm container after init-softhsm.sh completes:
#
#   docker-compose -f deploy/lifegw/vault-sidecar/docker-compose.yml \
#       exec softhsm /test-vectors/secp256k1-sign.sh
#
# This script does NOT exercise the (still-missing) PKCS#11→HTTP
# bridge between softhsm + the Vault transit/sign wire shape that
# VaultTransitAnima expects. That gap is documented in README.md +
# BRO-1215-followup-pkcs11-bridge.

set -euo pipefail

TOKEN_LABEL="${TOKEN_LABEL:-lifegw-anima}"
TOKEN_USER_PIN="${TOKEN_USER_PIN:-1234}"
KEY_LABEL="${KEY_LABEL:-rfc6979-spike-wallet}"
PKCS11_MODULE="${PKCS11_MODULE:-/usr/lib/softhsm/libsofthsm2.so}"

WORKDIR="${WORKDIR:-/tmp/lifegw-vault-sidecar-sign}"
mkdir -p "${WORKDIR}"

# Canonical message — must match
# crates/life-runtime/lifegw/tests/secp256k1_test_vector.rs::vector::MESSAGE.
MESSAGE="sample"

# Canonical SHA-256(message) — what the PKCS#11 sign call hashes-then-signs
# when invoked with --mechanism ECDSA (CKM_ECDSA expects a pre-hash digest).
# Must match vector::SHA256_OF_MESSAGE_HEX in the Rust test.
EXPECTED_DIGEST_HEX="AF2BDBE1AA9B6EC1E2ADE1D694F41FC71A831D0268E9891562113D8A62ADD1BF"

# Canonical r||s the Rust test pins. Must match vector::EXPECTED_RS_HEX.
EXPECTED_RS_HEX="432310E32CB80EB6503A26CE83CC165C783B870845FB8AAD6D970889FCD7A6C8530128B6B81C548874A6305D93ED071CA6E05074D85863D4056CE89B02BFAB69"

echo "[secp256k1-sign] step 1/4 — sanity check message digest"
printf '%s' "${MESSAGE}" > "${WORKDIR}/message.txt"
ACTUAL_DIGEST_HEX=$(openssl dgst -sha256 -binary "${WORKDIR}/message.txt" | xxd -p -u | tr -d '\n')
if [[ "${ACTUAL_DIGEST_HEX}" != "${EXPECTED_DIGEST_HEX}" ]]; then
  echo "[secp256k1-sign] FATAL: SHA-256('${MESSAGE}') mismatch" >&2
  echo "[secp256k1-sign]   expected: ${EXPECTED_DIGEST_HEX}" >&2
  echo "[secp256k1-sign]   actual  : ${ACTUAL_DIGEST_HEX}" >&2
  exit 1
fi
echo "[secp256k1-sign] OK — SHA-256 matches"

echo "[secp256k1-sign] step 2/4 — preparing digest binary for PKCS#11 CKM_ECDSA"
# CKM_ECDSA signs a *digest* (not raw bytes). Convert the hex digest
# back to binary and feed pkcs11-tool the binary.
printf '%s' "${EXPECTED_DIGEST_HEX}" | xxd -r -p > "${WORKDIR}/digest.bin"

echo "[secp256k1-sign] step 3/4 — signing via softhsm PKCS#11"
pkcs11-tool \
  --module "${PKCS11_MODULE}" \
  --token-label "${TOKEN_LABEL}" \
  --pin "${TOKEN_USER_PIN}" \
  --sign \
  --mechanism ECDSA \
  --label "${KEY_LABEL}" \
  -i "${WORKDIR}/digest.bin" \
  -o "${WORKDIR}/signature.bin"

# pkcs11-tool emits the signature in raw r||s form (no DER wrapping),
# exactly the encoding k256's `Signature::to_bytes()` produces.
ACTUAL_RS_HEX=$(xxd -p -u "${WORKDIR}/signature.bin" | tr -d '\n')

echo "[secp256k1-sign] step 4/4 — comparing against pinned k256 bytes"
echo "[secp256k1-sign]   expected: ${EXPECTED_RS_HEX}"
echo "[secp256k1-sign]   actual  : ${ACTUAL_RS_HEX}"

if [[ "${ACTUAL_RS_HEX}" == "${EXPECTED_RS_HEX}" ]]; then
  echo "[secp256k1-sign] OK — softhsm produces the expected RFC 6979 deterministic signature."
  echo "[secp256k1-sign] M9-E stopgap signing path is sound."
  exit 0
else
  echo "[secp256k1-sign] MISMATCH — softhsm did NOT produce the expected bytes." >&2
  echo "[secp256k1-sign] STOP. Do NOT proceed with the production deploy until this is resolved." >&2
  echo "[secp256k1-sign] Most likely causes:" >&2
  echo "[secp256k1-sign]   - softhsm built without secp256k1 OID support" >&2
  echo "[secp256k1-sign]   - softhsm version uses a non-RFC-6979 k generator" >&2
  echo "[secp256k1-sign]   - PKCS#11 module path mismatch (check PKCS11_MODULE env)" >&2
  exit 1
fi
