#!/usr/bin/env bash
# Verify life-kernel-* and facade/client/schema dependency rules.
#
# life-kernel-* may depend on:
#   aios-protocol, arcan-sandbox, arcan-provider-*, lago-core, life-vigil
# life-kernel-* must NOT depend on:
#   arcand, arcan-core, arcan-harness, arcan-aios-adapters
#
# life-kernel-facade (Phase 1) must NOT directly depend on runtime daemon crates.
# life-client (Phase 1) must NOT depend on life-kernel-* internal crates or lifed.
# *-api-schema crates must NOT depend on runtime crates (tokio, axum, reqwest, tonic, hyper).
#
# Runs from the monorepo root (core/life) — uses workspace `cargo tree`.

set -euo pipefail

# Resolve repo root relative to this script.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

FAIL=0

# check_no_dep CRATE FORBIDDEN
# Fails if CRATE has FORBIDDEN as a direct (depth-1) dependency.
# Silently skips if CRATE does not exist in the workspace yet.
check_no_dep() {
    local crate="$1"
    local forbidden="$2"
    local tree
    tree=$(cd "$ROOT_DIR" && cargo tree -p "$crate" --prefix depth 2>/dev/null) || return 0
    if echo "$tree" | grep -qE "^1[[:space:]]+${forbidden}( |$)"; then
        echo "FAIL: $crate has forbidden direct dependency on $forbidden"
        FAIL=1
    fi
}

# check_no_transitive_dep CRATE FORBIDDEN
# Fails if FORBIDDEN appears anywhere in the full dep tree of CRATE.
# Silently skips if CRATE does not exist in the workspace yet.
check_no_transitive_dep() {
    local crate="$1"
    local forbidden="$2"
    local tree
    tree=$(cd "$ROOT_DIR" && cargo tree -p "$crate" 2>/dev/null) || return 0
    if echo "$tree" | grep -qE "(^| )${forbidden}( |$)"; then
        echo "FAIL: $crate transitively depends on forbidden crate $forbidden"
        FAIL=1
    fi
}

# ── Rule set 1: life-kernel-* (existing) ─────────────────────────────────────
echo "=== life-kernel dependency rules ==="
for crate in life-kernel-proto life-kernel-core life-kernel-gate life-kernel-conformance; do
    check_no_dep "$crate" "arcand"
    check_no_dep "$crate" "arcan-core"
    check_no_dep "$crate" "arcan-harness"
    check_no_dep "$crate" "arcan-aios-adapters"
done

# ── Rule set 2: life-kernel-facade (Phase 1 — dormant until crate exists) ────
# life-kernel-facade must not directly depend on runtime daemon crates.
FACADE_FORBIDDEN_RUNTIME=(
    arcand arcan-core arcan-harness arcan-aios-adapters
    lago-core lago-journal lago-store lago-knowledge lago-lance lago-fs
    autonomic-controller autonomic-core
    haima-core haima-wallet haima-x402 haima-lago
    nous-core nous-judge nous-heuristics
    opsis-core opsis-engine
    anima-core anima-identity
    life-relay-core
)
echo "=== life-kernel-facade dependency rules (dormant until Phase 1) ==="
for forbidden in "${FACADE_FORBIDDEN_RUNTIME[@]}"; do
    check_no_dep "life-kernel-facade" "$forbidden"
done

# ── Rule set 3: life-client (Phase 1 — dormant until crate exists) ───────────
# life-client must not depend on life-kernel internal crates or lifed.
CLIENT_FORBIDDEN=(
    life-kernel-core life-kernel-gate life-kernel-facade lifed
)
echo "=== life-client dependency rules (dormant until Phase 1) ==="
for forbidden in "${CLIENT_FORBIDDEN[@]}"; do
    check_no_dep "life-client" "$forbidden"
done

# ── Rule set 4: *-api-schema crates must not pull in runtime crates ──────────
SCHEMA_CRATES=(
    arcan-api-schema
    autonomic-api-schema
    haima-api-schema
    lago-api-schema
    life-relay-api-schema
    nous-api-schema
    opsis-api-schema
)
SCHEMA_FORBIDDEN_RUNTIME=(
    tokio axum reqwest tonic hyper
    arcand lagod autonomicd haimad nousd opsisd life-relayd lifed
)
echo "=== *-api-schema runtime isolation rules ==="
for schema_crate in "${SCHEMA_CRATES[@]}"; do
    for forbidden in "${SCHEMA_FORBIDDEN_RUNTIME[@]}"; do
        check_no_transitive_dep "$schema_crate" "$forbidden"
    done
done

if [ "$FAIL" -eq 0 ]; then
    echo "=== all dependency rules passed ==="
else
    exit 1
fi
