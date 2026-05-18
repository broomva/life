#!/usr/bin/env bash
# Verify lifed + *-proxy + life-runtime-proto dependency rules per Spec C₂ §11.
#
# lifed (the facade-aggregator daemon) MAY depend on:
#   - aios-protocol, aios-proto, life-runtime-proto
#   - life-kernel-proto (client features only — for the SpawnChild saga's
#     soma admin call)
#   - the four *-proxy crates (arcan-proxy, lago-proxy, haima-proxy, anima-proxy)
#   - life-vigil
#   - transport / utility crates (tonic, tower, jsonwebtoken, etc.)
#
# lifed MUST NOT depend on any substrate runtime crate:
#   - arcand, arcan-core, arcan-harness, arcan-aios-adapters
#   - lago-runtime (lago-core, lago-journal, lago-store, etc.)
#   - haima-runtime (haima-core, haima-wallet, haima-x402, etc.)
#   - anima-runtime (anima-core, anima-identity, anima-lago, etc.)
#   - life-kernel-core, life-kernel-gate, life-kernel-facade
#   - arcan-provider-* (these belong to arcan, not lifed)
#
# The four *-proxy crates depend ONLY on:
#   - aios-protocol, aios-proto
#   - the substrate's wire crate (life-kernel-proto where applicable)
#   - tonic + light utility crates (async-trait, futures, tokio-stream, etc.)
# They MUST NOT pull substrate runtime crates either.
#
# Runs from the monorepo root (core/life) — uses workspace `cargo tree`.
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

FAIL=0

# check_no_transitive_dep CRATE FORBIDDEN_REGEX
# Fails if any crate matching FORBIDDEN_REGEX appears anywhere in the full
# dep tree of CRATE. Silently skips if CRATE does not exist yet.
#
# BRO-1164: uses here-string (`<<<`) instead of `echo | grep` to avoid
# SIGPIPE on the echo side when grep -q matches early. Hits are limited
# via `grep -m3` (also no pipe) for the same reason.
check_no_transitive_dep() {
    local crate="$1"
    local forbidden_regex="$2"
    local tree
    tree=$(cd "$ROOT_DIR" && cargo tree -p "$crate" 2>/dev/null) || return 0
    # Match crate names at the start of a node (after the └── / ├── prefix
    # and version), to avoid matching docstrings or feature names.
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
fake-lifed v0.3.0 (/tmp/fake)
├── aios-protocol v0.1.0
├── lago-journal v0.3.0 (/tmp/fake/lago-journal)
└── tonic v0.14.0
FAKE_TREE
    }
    export -f cargo
    FAIL=0
    check_no_transitive_dep "fake-lifed" "lago-journal"
    if [ "$FAIL" -eq 1 ]; then
        echo "OK: self-test passed — FAIL path fires when forbidden crate present"
        exit 0
    else
        echo "REGRESSION: self-test FAILED — FAIL path did not fire (BRO-1164 returned)"
        exit 2
    fi
fi

echo "=== lifed + life-runtime-* dependency rules (Spec C₂ §11) ==="

# lifed itself + every proxy must avoid every substrate runtime crate.
for crate in lifed arcan-proxy lago-proxy haima-proxy anima-proxy life-runtime-proto lifed-conformance life-runtime-pool; do
    # Skip with a quiet message if the crate hasn't landed yet (sub-phase B
    # introduces the real proxy bodies).
    if ! (cd "$ROOT_DIR" && cargo metadata --no-deps --format-version 1 \
            | grep -qE "\"name\":\"${crate}\""); then
        echo "skip: ${crate} not in workspace yet"
        continue
    fi

    # Arcan substrate runtime crates.
    check_no_transitive_dep "$crate" "arcand|arcan-core|arcan-harness|arcan-aios-adapters"
    # Arcan providers — substrate-internal sandbox/launchers.
    check_no_transitive_dep "$crate" "arcan-provider-bubblewrap|arcan-provider-local|arcan-provider-vercel|arcan-sandbox"
    # Lago substrate runtime crates.
    check_no_transitive_dep "$crate" "lago-core|lago-journal|lago-store|lago-fs|lago-policy|lago-knowledge|lago-auth|lago-api|lago-ingest|lago-aios-eventstore-adapter|lago-cli|lagod|lago-billing|lago-compiler|lago-lance"
    # Haima substrate runtime crates.
    check_no_transitive_dep "$crate" "haima-core|haima-wallet|haima-x402|haima-lago|haima-api|haimad|haima-insurance|haima-outcome"
    # Anima substrate runtime crates.
    check_no_transitive_dep "$crate" "anima-core|anima-identity|anima-lago"
    # Life-kernel internals (only life-kernel-proto is allowed and only for lifed).
    check_no_transitive_dep "$crate" "life-kernel-core|life-kernel-gate|life-kernel-facade"
    # The proxy crates additionally must not pull life-kernel-proto.
    if [ "$crate" != "lifed" ]; then
        check_no_transitive_dep "$crate" "life-kernel-proto"
    fi
done

if [ $FAIL -ne 0 ]; then
    echo
    echo "Dependency rules violated. See Spec C₂ §11 for the rationale."
    exit 1
fi
echo "all lifed dependency rules pass"
