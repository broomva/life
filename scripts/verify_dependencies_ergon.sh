#!/usr/bin/env bash
# Verify ergon + ergon-life-* dependency rules per
# `core/life/docs/superpowers/specs/2026-05-05-ergon-v0.1.md` §3 and the
# crate-level CLAUDE.md invariants.
#
# Ergon is the workflow primitive at Layer 2 of the agent-harness stack
# (`core/life/docs/architecture/agent-harness.md`). Its layering rules are
# narrower than lifed's:
#
#   - `ergon` (the core crate) is VENDOR-NEUTRAL. Zero Life-substrate runtime
#     deps. Pulls only:
#       - aios-protocol (kernel contract)
#       - async/std/serde/thiserror/tokio (sync features only)/tracing
#       - gray_matter, jsonschema (authored-agent substrate, BRO-1007)
#
#     `ergon` MUST NOT depend on any of:
#       - arcan-* (runtime — adapter lives in `arcan-ergon` instead)
#       - praxis-* (tool substrate — `ToolRegistry` trait is ergon-owned)
#       - anima-*, autonomic-*, nous-* (auto-hook concerns)
#       - lago-* (event journal — handled by the adapter via `EventStorePort`)
#       - life-vigil (observability — sinks emit via `tracing` directly)
#       - life-kernel-*, life-runtime-* (gateway/daemon concerns)
#
#   - `ergon-life-hooks` is LIFE-COUPLED. May depend on the 4 substrate
#     APIs the auto-hooks bridge: praxis-core, autonomic, anima, nous.
#     MUST NOT pull arcan-*, lago-journal, life-vigil, life-runtime-*.
#
#   - `ergon-life-sinks` is LIFE-COUPLED on the persistence path. May
#     depend on lago-core (Journal trait). MUST NOT pull life-vigil
#     (sinks emit via `tracing` directly), arcan-*, praxis-*, anima-*,
#     autonomic-*, nous-*, life-kernel-*, life-runtime-*.
#
# Runs from the monorepo root (core/life) — uses workspace `cargo tree`.
#
# Mirrors the SIGPIPE-safe `<<<` here-string pattern from
# `verify_dependencies_lifed.sh` [BRO-1164].

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
# check function to prove the FAIL path fires.
if [ "${1:-}" = "--self-test" ]; then
    echo "=== SELF-TEST: verify FAIL detection still works ==="
    cargo() {
        cat <<'FAKE_TREE'
fake-ergon v0.3.0 (/tmp/fake)
├── aios-protocol v0.3.0
├── arcan-core v0.3.0 (/tmp/fake/arcan-core)
└── tonic v0.14.0
FAKE_TREE
    }
    export -f cargo
    FAIL=0
    check_no_transitive_dep "fake-ergon" "arcan-core"
    if [ "$FAIL" -eq 1 ]; then
        echo "OK: self-test passed — FAIL path fires when forbidden crate present"
        exit 0
    else
        echo "REGRESSION: self-test FAILED — FAIL path did not fire"
        exit 2
    fi
fi

echo "=== ergon + ergon-life-* dependency rules (Spec ergon-v0.1 §3) ==="

# Rule set 1 — `ergon` (the core crate): zero substrate runtime deps.
#
# Forbidden = every Life substrate runtime crate + every adapter shim that
# would create coupling. The crate's CLAUDE.md §"Invariants" pins this to
# zero dependencies on lago-journal / life-vigil / praxis-* / arcan-* /
# anima-* / autonomic-* / nous-*. Spec ergon-v0.1 §3 + the BRO-997 wire-
# types decision (CLAUDE.md "Spec deviations" item 3) lock the same rule.
echo
echo "-- rule 1: ergon (core) is vendor-neutral --"

# Arcan runtime + adapters.
check_no_transitive_dep "ergon" "arcand|arcan-core|arcan-harness|arcan-aios-adapters|arcan-anima|arcan-lago|arcan-praxis|arcan-provider|arcan-provider-bubblewrap|arcan-provider-cube|arcan-provider-local|arcan-provider-vercel|arcan-sandbox|arcan-ergon"
# Praxis substrate.
check_no_transitive_dep "ergon" "praxis-core|praxis-skills|praxis-tools"
# Anima substrate.
check_no_transitive_dep "ergon" "anima-core|anima-identity|anima-lago"
# Autonomic substrate.
check_no_transitive_dep "ergon" "autonomic-core|autonomic-runtime"
# Nous substrate.
check_no_transitive_dep "ergon" "nous-core|nous-judge|nous-runtime"
# Lago substrate (the core crate does not even pull lago-core — only the
# sinks crate does).
check_no_transitive_dep "ergon" "lago-core|lago-journal|lago-store|lago-fs|lago-policy|lago-knowledge|lago-auth|lago-api|lago-ingest|lago-billing|lago-compiler|lago-lance"
# Life observability — vigil's tracing-subscriber is global, not a sink dep.
check_no_transitive_dep "ergon" "life-vigil"
# Life-kernel internals + runtime gateways.
check_no_transitive_dep "ergon" "life-kernel-core|life-kernel-gate|life-kernel-facade|life-kernel-proto"
check_no_transitive_dep "ergon" "lifed|lifegw|arcan-proxy|lago-proxy|haima-proxy|anima-proxy|life-runtime-proto|life-runtime-pool"

# Rule set 2 — `ergon-life-hooks`: may bridge to the 4 auto-hook
# substrates (praxis-core, autonomic, anima, nous), but stays out of
# arcan runtime, lago journal, life-vigil, and the runtime gateways.
echo
echo "-- rule 2: ergon-life-hooks bridges only the 4 auto-hook substrates --"

check_no_transitive_dep "ergon-life-hooks" "arcand|arcan-core|arcan-harness|arcan-aios-adapters|arcan-ergon|arcan-praxis|arcan-anima|arcan-lago|arcan-provider|arcan-provider-bubblewrap|arcan-provider-cube|arcan-provider-local|arcan-provider-vercel|arcan-sandbox"
check_no_transitive_dep "ergon-life-hooks" "lago-journal|lago-store|lago-knowledge|lago-policy|lago-fs|lago-api|lago-ingest|lago-billing|lago-compiler|lago-lance"
check_no_transitive_dep "ergon-life-hooks" "life-vigil"
check_no_transitive_dep "ergon-life-hooks" "lifed|lifegw|arcan-proxy|lago-proxy|haima-proxy|anima-proxy|life-runtime-proto|life-runtime-pool|life-kernel-core|life-kernel-gate|life-kernel-facade|life-kernel-proto"

# Rule set 3 — `ergon-life-sinks`: may depend on lago-core (Journal
# trait) but stays out of arcan/praxis/anima/autonomic/nous + lago
# runtime impls + the runtime gateways. Vigil specifically forbidden
# (per `ergon-life-sinks/CLAUDE.md` §"Dependencies (locked)").
echo
echo "-- rule 3: ergon-life-sinks depends only on lago-core + tracing --"

check_no_transitive_dep "ergon-life-sinks" "arcand|arcan-core|arcan-harness|arcan-aios-adapters|arcan-ergon|arcan-praxis|arcan-anima|arcan-lago|arcan-provider|arcan-provider-bubblewrap|arcan-provider-cube|arcan-provider-local|arcan-provider-vercel|arcan-sandbox"
check_no_transitive_dep "ergon-life-sinks" "praxis-core|praxis-skills|praxis-tools"
check_no_transitive_dep "ergon-life-sinks" "anima-core|anima-identity|anima-lago"
check_no_transitive_dep "ergon-life-sinks" "autonomic-core|autonomic-runtime"
check_no_transitive_dep "ergon-life-sinks" "nous-core|nous-judge|nous-runtime"
check_no_transitive_dep "ergon-life-sinks" "lago-journal|lago-store|lago-knowledge|lago-policy|lago-fs|lago-api|lago-ingest|lago-billing|lago-compiler|lago-lance|lago-auth"
check_no_transitive_dep "ergon-life-sinks" "life-vigil"
check_no_transitive_dep "ergon-life-sinks" "lifed|lifegw|arcan-proxy|lago-proxy|haima-proxy|anima-proxy|life-runtime-proto|life-runtime-pool|life-kernel-core|life-kernel-gate|life-kernel-facade|life-kernel-proto"

if [ $FAIL -ne 0 ]; then
    echo
    echo "Ergon dependency rules violated. See:"
    echo "  - core/life/docs/superpowers/specs/2026-05-05-ergon-v0.1.md §3"
    echo "  - core/life/crates/ergon/ergon/CLAUDE.md §Invariants"
    echo "  - core/life/crates/ergon/ergon-life-sinks/CLAUDE.md §Dependencies"
    exit 1
fi
echo
echo "all ergon dependency rules pass"
