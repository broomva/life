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
#
# SIGPIPE bug fix [BRO-1164]: this script previously used
#   `echo "$tree" | grep -qE "..."`
# under `set -o pipefail`. When grep -q matched early and closed the read end
# of the pipe, echo received SIGPIPE on the next write and the pipeline exited
# non-zero (signal 13). The `if` statement interpreted that as "no match" —
# silently masking real FAILs on CI Linux runners. Replaced with `<<<` here-
# strings (no pipe, no SIGPIPE).

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE="lifegw-anthropic-codec"

FAIL=0

# check_no_transitive_dep CRATE FORBIDDEN_REGEX
# Fails if any crate matching FORBIDDEN_REGEX appears anywhere in the
# full dep tree of CRATE.
#
# BRO-1164: uses here-string (`<<<`) instead of `echo | grep` to avoid
# SIGPIPE on the echo side when grep -q matches early. Hits are limited
# via `grep -m3` (also no pipe) for the same reason.
check_no_transitive_dep() {
    local crate="$1"
    local forbidden_regex="$2"
    local tree
    tree=$(cd "$ROOT_DIR" && cargo tree -p "$crate" 2>/dev/null) || return 0
    if grep -qE "[ ]${forbidden_regex} v" <<< "$tree"; then
        local hits
        hits=$(grep -m3 -E "[ ]${forbidden_regex} v" <<< "$tree")
        echo "FAIL: $crate transitively depends on a crate matching ${forbidden_regex}:"
        sed 's/^/    /' <<< "$hits"
        FAIL=1
    fi
}

# --self-test: bypasses cargo and feeds a synthetic forbidden tree through the
# check function to prove the FAIL path fires. BRO-1164 root cause was a silent
# masking of FAILs; this asserts the regression cannot recur.
if [ "${1:-}" = "--self-test" ]; then
    echo "=== SELF-TEST: verify FAIL detection still works (BRO-1164 regression guard) ==="
    cargo() {
        cat <<'FAKE_TREE'
fake-codec v0.1.0 (/tmp/fake)
├── serde v1.0.0
├── inference-core v0.1.0 (/tmp/fake/inference-core)
└── tokio v1.0.0
FAKE_TREE
    }
    export -f cargo
    FAIL=0
    check_no_transitive_dep "fake-codec" "inference-core"
    if [ "$FAIL" -eq 1 ]; then
        echo "OK: self-test passed — FAIL path fires when forbidden crate present"
        exit 0
    else
        echo "REGRESSION: self-test FAILED — FAIL path did not fire (BRO-1164 returned)"
        exit 2
    fi
fi

echo "=== lifegw-anthropic-codec dependency rules (Spec J L10-D1) ==="

# Skip with a quiet message if the crate hasn't landed yet.
metadata_json="$(cd "$ROOT_DIR" && cargo metadata --no-deps --format-version 1)"
if ! grep -qE "\"name\":\"${CRATE}\"" <<< "$metadata_json"; then
    echo "skip: ${CRATE} not in workspace yet"
    exit 0
fi
unset metadata_json

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
