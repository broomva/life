#!/usr/bin/env bash
# Verify lifegw dependency rules per Spec C₃ §11.
#
# lifegw (the unprivileged stateless edge gateway daemon) MAY depend on:
#   - aios-protocol, aios-proto, life-runtime-proto (Spec C₂ public-plane proto types)
#   - life-kernel-proto (wire types ONLY, for Spec D D-Sub-C anima custody routes
#     that proxy to soma's `life.admin.kernel.v1.CustodyOracle` admin service;
#     this is symmetric with lifed's life-kernel-proto allowance for its
#     SpawnChild saga — both daemons consume the wire types but never the
#     runtime crates)
#   - life-vigil (observability)
#   - transport / TLS / WS / JWT crates (tonic, tonic-web, rustls, jsonwebtoken, etc.)
#   - standard utility crates (anyhow, thiserror, clap, toml, tower, tower-http, etc.)
#
# lifegw MUST NOT depend on (Spec C₃ §11.2 LOCKED L4-D13):
#   - substrate runtime crates: arcand, arcan-core, arcan-harness, arcan-aios-adapters,
#     arcan-provider-* (substrate-internal sandbox/launchers),
#     lago-runtime family (lago-core, lago-journal, lago-store, etc.),
#     haima-runtime family (haima-core, haima-wallet, haima-x402, etc.),
#     anima-runtime family (anima-core, anima-identity, anima-lago, etc.),
#     life-kernel-* internals (life-kernel-core, life-kernel-gate, life-kernel-facade —
#     proto is allowed per the carve-out above; the runtime/facade crates are not)
#   - substrate proxy crates: arcan-proxy, lago-proxy, haima-proxy, anima-proxy
#     (these are lifed's south-side; the gateway never reaches a substrate directly)
#   - lifed itself (the gateway dials via life-runtime-proto's tonic client; it
#     must not link against lifed's saga, routing-cache, or auth code)
#
# A substrate panic must never reach the gateway. Bypassing lifed would skip
# the Tier-2 → Tier-3 derivation. Both are non-negotiable trust-boundary rules.
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
    # `--edges normal` excludes dev-dependencies and build-dependencies so the
    # check inspects only the *production* dep graph. Spec C₃ §11.2 forbids
    # production deps on substrate runtimes / proxies / lifed; integration
    # tests are allowed to pull lifed as a dev-dep to stand up an end-to-end
    # rig (see crates/life-runtime/lifegw/tests/integration_proxy_passthrough.rs).
    tree=$(cd "$ROOT_DIR" && cargo tree -p "$crate" --edges normal 2>/dev/null) || return 0
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
    # Override `cargo` to emit a fake tree containing a forbidden crate.
    cargo() {
        cat <<'FAKE_TREE'
fake-lifegw v0.3.0 (/tmp/fake)
├── aios-protocol v0.1.0
├── arcan-core v0.3.0 (/tmp/fake/arcan-core)
└── tonic v0.14.0
FAKE_TREE
    }
    export -f cargo
    FAIL=0
    check_no_transitive_dep "fake-lifegw" "arcan-core"
    if [ "$FAIL" -eq 1 ]; then
        echo "OK: self-test passed — FAIL path fires when forbidden crate present"
        exit 0
    else
        echo "REGRESSION: self-test FAILED — FAIL path did not fire (BRO-1164 returned)"
        exit 2
    fi
fi

echo "=== lifegw (edge gateway) dependency rules (Spec C₃ §11) ==="

# Skip with a quiet message if the crate hasn't landed yet.
metadata_json="$(cd "$ROOT_DIR" && cargo metadata --no-deps --format-version 1)"
if ! grep -qE "\"name\":\"lifegw\"" <<< "$metadata_json"; then
    echo "skip: lifegw not in workspace yet"
    exit 0
fi
unset metadata_json

# Arcan substrate runtime crates.
check_no_transitive_dep "lifegw" "arcand|arcan-core|arcan-harness|arcan-aios-adapters"
# Arcan providers — substrate-internal sandbox/launchers.
check_no_transitive_dep "lifegw" "arcan-provider-bubblewrap|arcan-provider-local|arcan-provider-vercel|arcan-sandbox"
# Lago substrate runtime crates.
check_no_transitive_dep "lifegw" "lago-core|lago-journal|lago-store|lago-fs|lago-policy|lago-knowledge|lago-auth|lago-api|lago-ingest|lago-aios-eventstore-adapter|lago-cli|lagod|lago-billing|lago-compiler|lago-lance"
# Haima substrate runtime crates.
check_no_transitive_dep "lifegw" "haima-core|haima-wallet|haima-x402|haima-lago|haima-api|haimad|haima-insurance|haima-outcome"
# Anima substrate runtime crates.
check_no_transitive_dep "lifegw" "anima-core|anima-identity|anima-lago"
# Life-kernel internals — lifegw must NOT pull the runtime crates. Note: life-kernel-proto
# IS allowed per Spec D D-Sub-C (anima custody routes proxy to soma's
# `life.admin.kernel.v1.CustodyOracle` and need the typed wire types). This mirrors
# lifed's carve-out for SpawnChild's soma admin call — both daemons consume the
# wire types but never link the runtime/facade crates. See `services/anima_custody.rs`.
check_no_transitive_dep "lifegw" "life-kernel-core|life-kernel-gate|life-kernel-facade"
# Substrate proxy crates — owned by lifed's south side; lifegw must not reach
# substrates directly (master spec §L13 anti-pattern #10).
check_no_transitive_dep "lifegw" "arcan-proxy|lago-proxy|haima-proxy|anima-proxy"
# Lifed runtime crate — the gateway dials via life-runtime-proto's tonic
# client and must never link against lifed's saga / routing / auth code.
check_no_transitive_dep "lifegw" "lifed"

if [ $FAIL -ne 0 ]; then
    echo
    echo "Dependency rules violated. See Spec C₃ §11 for the rationale:"
    echo "  docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md"
    exit 1
fi
echo "all lifegw dependency rules pass"
