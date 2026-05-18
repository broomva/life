#!/usr/bin/env bash
# Verify life-kernel-* and facade/client/schema dependency rules.
#
# life-kernel-* may depend on:
#   aios-protocol, arcan-sandbox, arcan-provider-*, lago-core, life-vigil
# life-kernel-* must NOT depend on:
#   arcand, arcan-core, arcan-harness, arcan-aios-adapters
#
# life-kernel-facade (Phase 1) must NOT directly depend on runtime daemon crates.
# life-client (Phase 1) must NOT depend on life-kernel-* internal crates or soma.
# *-api-schema crates must NOT depend on runtime crates (tokio, axum, reqwest, tonic, hyper).
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

# Resolve repo root relative to this script.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

FAIL=0

# check_no_dep CRATE FORBIDDEN
# Fails if CRATE has FORBIDDEN as a direct (depth-1) dependency.
# Silently skips if CRATE does not exist in the workspace yet.
#
# BRO-1164: uses here-string (`<<<`) instead of `echo | grep` to avoid
# SIGPIPE on the echo side when grep -q matches early.
check_no_dep() {
    local crate="$1"
    local forbidden="$2"
    local tree
    tree=$(cd "$ROOT_DIR" && cargo tree -p "$crate" --prefix depth 2>/dev/null) || return 0
    if grep -qE "^1[[:space:]]+${forbidden}( |$)" <<< "$tree"; then
        echo "FAIL: $crate has forbidden direct dependency on $forbidden"
        FAIL=1
    fi
}

# check_no_transitive_dep CRATE FORBIDDEN
# Fails if FORBIDDEN appears anywhere in the full dep tree of CRATE.
# Silently skips if CRATE does not exist in the workspace yet.
#
# BRO-1164: uses here-string (`<<<`) instead of `echo | grep` to avoid
# SIGPIPE on the echo side when grep -q matches early.
check_no_transitive_dep() {
    local crate="$1"
    local forbidden="$2"
    local tree
    tree=$(cd "$ROOT_DIR" && cargo tree -p "$crate" 2>/dev/null) || return 0
    if grep -qE "(^| )${forbidden}( |$)" <<< "$tree"; then
        echo "FAIL: $crate transitively depends on forbidden crate $forbidden"
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
0 fake-soma v0.1.0
1 arcan-core v0.3.0
2 lago-core v0.3.0
FAKE_TREE
    }
    export -f cargo
    FAIL=0
    # The actual scripts use two different check functions; exercise both.
    check_no_dep "fake-soma" "arcan-core"
    if [ "$FAIL" -ne 1 ]; then
        echo "REGRESSION: check_no_dep self-test FAILED — FAIL path did not fire (BRO-1164 returned)"
        exit 2
    fi
    # Reset and exercise the transitive variant. Override cargo with a tree
    # in the format the transitive check expects (no depth prefix).
    cargo() {
        cat <<'FAKE_TREE'
fake-soma v0.1.0 (/tmp/fake)
├── aios-protocol v0.1.0
└── tokio v1.0.0
FAKE_TREE
    }
    export -f cargo
    FAIL=0
    check_no_transitive_dep "fake-soma" "tokio"
    if [ "$FAIL" -eq 1 ]; then
        echo "OK: self-test passed — both FAIL paths fire when forbidden crate present"
        exit 0
    else
        echo "REGRESSION: check_no_transitive_dep self-test FAILED (BRO-1164 returned)"
        exit 2
    fi
fi

# ── Rule set 1: life-kernel-* (existing) ─────────────────────────────────────
echo "=== life-kernel dependency rules ==="
# Library-tier crates: must not depend on runtime/adapter crates.
for crate in life-kernel-proto life-kernel-core life-kernel-gate life-kernel-conformance; do
    check_no_dep "$crate" "arcand"
    check_no_dep "$crate" "arcan-core"
    check_no_dep "$crate" "arcan-harness"
    check_no_dep "$crate" "arcan-aios-adapters"
done
# Binary-tier crate (soma): also must not pull in the
# runtime/adapter crates (it depends on arcan-provider-* via
# life-kernel-core, but not on arcand/arcan-core/arcan-harness/arcan-aios-adapters).
for crate in soma; do
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
# life-client must not depend on life-kernel internal crates or soma.
CLIENT_FORBIDDEN=(
    life-kernel-core life-kernel-gate life-kernel-facade soma
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
    arcand lagod autonomicd haimad nousd opsisd life-relayd soma
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
