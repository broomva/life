#!/usr/bin/env bash
# Verify life-kernel-* dependency rules.
#
# life-kernel-* may depend on:
#   aios-protocol, arcan-sandbox, arcan-provider-*, lago-core, life-vigil
# life-kernel-* must NOT depend on:
#   arcand, arcan-core, arcan-harness, arcan-aios-adapters
#
# Runs from the monorepo root (core/life) — uses workspace `cargo tree`.

set -euo pipefail

# Resolve repo root relative to this script.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

FAIL=0

check_no_dep() {
    local crate="$1"
    local forbidden="$2"
    # `cargo tree -p <crate> --prefix depth` prefixes each line with an integer
    # indentation level; depth "1" lines are direct dependencies.
    if (cd "$ROOT_DIR" && cargo tree -p "$crate" --prefix depth 2>/dev/null) \
         | grep -qE "^ *1 *${forbidden}( |$)"; then
        echo "FAIL: $crate has forbidden direct dependency on $forbidden"
        FAIL=1
    fi
}

echo "=== life-kernel dependency rules ==="
for crate in life-kernel-proto life-kernel-core life-kernel-gate life-kernel-conformance; do
    check_no_dep "$crate" "arcand"
    check_no_dep "$crate" "arcan-core"
    check_no_dep "$crate" "arcan-harness"
    check_no_dep "$crate" "arcan-aios-adapters"
done

if [ "$FAIL" -eq 0 ]; then
    echo "=== life-kernel dependency rules passed ==="
else
    exit 1
fi
