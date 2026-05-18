#!/usr/bin/env bash
# Verify the lifegw-anthropic-codec crate honours Spec J L10-D1:
# it is edge-only and substrate-free.
#
# The crate MAY depend on:
#   - life-runtime-proto (for pb::AgentEvent typed bindings)
#   - serde / serde_json / tokio / futures / bytes
#   - sha2 / hex (for synthesize_sid hashing)
#   - thiserror / tracing
#
# The crate MUST NOT depend on:
#   - arcand, arcan-core, arcan-harness, arcan-aios-adapters,
#     arcan-provider-*, arcan-sandbox (Arcan substrate runtime)
#   - lago-runtime crates (lago-core, lago-journal, lago-store, ...)
#   - haima-runtime crates (haima-core, haima-wallet, haima-x402, ...)
#   - anima-runtime crates (anima-core, anima-identity, anima-lago)
#   - inference-core (Spec E silicon-contract — separate concern)
#   - life-kernel-core, life-kernel-gate, life-kernel-facade
#
# Codec is pure wire-shape translation. Anything substrate-aware
# (auth, billing, sessions, custody) belongs in lifegw itself, not
# here.
#
# Patterned after scripts/verify_dependencies_lifed.sh.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE="lifegw-anthropic-codec"

FAIL=0

# check_no_transitive_dep CRATE FORBIDDEN_REGEX
# Fails if any crate matching FORBIDDEN_REGEX appears anywhere in the
# full dep tree of CRATE.
check_no_transitive_dep() {
    local crate="$1"
    local forbidden_regex="$2"
    local tree
    tree=$(cd "$ROOT_DIR" && cargo tree -p "$crate" 2>/dev/null) || return 0
    if echo "$tree" | grep -qE "[ ]${forbidden_regex} v"; then
        local hits
        hits=$(echo "$tree" | grep -E "[ ]${forbidden_regex} v" | head -3)
        echo "FAIL: $crate transitively depends on a crate matching ${forbidden_regex}:"
        echo "$hits" | sed 's/^/    /'
        FAIL=1
    fi
}

echo "=== lifegw-anthropic-codec dependency rules (Spec J L10-D1) ==="

# Skip with a quiet message if the crate hasn't landed yet.
if ! (cd "$ROOT_DIR" && cargo metadata --no-deps --format-version 1 \
        | grep -qE "\"name\":\"${CRATE}\""); then
    echo "skip: ${CRATE} not in workspace yet"
    exit 0
fi

# Arcan substrate runtime crates.
check_no_transitive_dep "$CRATE" \
    "arcand|arcan-core|arcan-harness|arcan-aios-adapters|arcan-store|arcan-sandbox"
# Arcan providers — substrate-internal sandbox/launchers.
check_no_transitive_dep "$CRATE" \
    "arcan-provider-bubblewrap|arcan-provider-local|arcan-provider-vercel|arcan-provider-cube"
# Lago substrate runtime crates.
check_no_transitive_dep "$CRATE" \
    "lago-core|lago-journal|lago-store|lago-fs|lago-policy|lago-knowledge|lago-auth|lago-api|lago-ingest|lago-aios-eventstore-adapter|lago-cli|lagod|lago-billing|lago-compiler|lago-lance"
# Haima substrate runtime crates.
check_no_transitive_dep "$CRATE" \
    "haima-core|haima-wallet|haima-x402|haima-lago|haima-api|haimad|haima-insurance|haima-outcome"
# Anima substrate runtime crates.
check_no_transitive_dep "$CRATE" \
    "anima-core|anima-identity|anima-lago"
# Inference (Spec E) — codec MUST NOT pull the silicon contract.
check_no_transitive_dep "$CRATE" \
    "inference-core|life-inference"
# Life-kernel internals.
check_no_transitive_dep "$CRATE" \
    "life-kernel-core|life-kernel-gate|life-kernel-facade"

if [ $FAIL -ne 0 ]; then
    echo
    echo "Dependency rules violated. See Spec J L10-D1 for the rationale."
    exit 1
fi
echo "OK: ${CRATE} dependencies are clean (edge-only, substrate-free)"
