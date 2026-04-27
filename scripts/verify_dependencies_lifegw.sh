#!/usr/bin/env bash
# Verify lifegw dependency rules per Spec C₃ §11.
#
# lifegw (the unprivileged stateless edge gateway daemon) MAY depend on:
#   - aios-protocol, aios-proto, life-runtime-proto (Spec C₂ public-plane proto types)
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
#     life-kernel-* (life-kernel-core, life-kernel-gate, life-kernel-facade,
#     life-kernel-proto)
#   - substrate proxy crates: arcan-proxy, lago-proxy, haima-proxy, anima-proxy
#     (these are lifed's south-side; the gateway never reaches a substrate directly)
#   - lifed itself (the gateway dials via life-runtime-proto's tonic client; it
#     must not link against lifed's saga, routing-cache, or auth code)
#
# A substrate panic must never reach the gateway. Bypassing lifed would skip
# the Tier-2 → Tier-3 derivation. Both are non-negotiable trust-boundary rules.
#
# Runs from the monorepo root (core/life) — uses workspace `cargo tree`.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

FAIL=0

# check_no_transitive_dep CRATE FORBIDDEN_REGEX
# Fails if any crate matching FORBIDDEN_REGEX appears anywhere in the full
# dep tree of CRATE. Silently skips if CRATE does not exist yet.
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
    if echo "$tree" | grep -qE "[ ]${forbidden_regex} v"; then
        local hits
        hits=$(echo "$tree" | grep -E "[ ]${forbidden_regex} v" | head -3)
        echo "FAIL: $crate transitively depends on a crate matching ${forbidden_regex}:"
        echo "$hits" | sed 's/^/    /'
        FAIL=1
    fi
}

echo "=== lifegw (edge gateway) dependency rules (Spec C₃ §11) ==="

# Skip with a quiet message if the crate hasn't landed yet.
if ! (cd "$ROOT_DIR" && cargo metadata --no-deps --format-version 1 \
        | grep -qE "\"name\":\"lifegw\""); then
    echo "skip: lifegw not in workspace yet"
    exit 0
fi

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
# Life-kernel internals — lifegw must NOT pull any of these (unlike lifed
# which is allowed life-kernel-proto for the SpawnChild saga).
check_no_transitive_dep "lifegw" "life-kernel-core|life-kernel-gate|life-kernel-facade|life-kernel-proto"
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
