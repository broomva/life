#!/usr/bin/env bash
#
# verify_dependencies_chronos.sh
#
# Enforces the chronos crate dependency rules locked by the M0 plan
# (`docs/superpowers/plans/2026-05-13-chronos-temporal-primitive.md` §"Dependency rules").
#
#   chronos-core      → aios-protocol ONLY  (no other internal Life crates; tokio/serde/etc OK)
#   chronos-triggers  → chronos-core ONLY  (depends on chronos-core; no other internal Life crates)
#   chronos-lago      → chronos-core + aios-protocol + lago-core  (lago-journal allowed for dev only)
#   chronos-api       → chronos-core ONLY  (axum/serde/tokio OK; concrete store + trigger injected)
#   chronosd          → chronos-* + aios-protocol + lago-*        (no arcan/autonomic/haima/etc)
#
# Dev-dependencies are intentionally exempt — the rules constrain the PRODUCTION dep graph.
# Mirror this style when adding new substrate primitives (e.g. aegis, nous expansion).
#
# Exit code 0 on pass, 1 on rule violation.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

TMP_JSON="$(mktemp -t chronos-deps.XXXXXX)"
trap 'rm -f "$TMP_JSON"' EXIT

cargo metadata --format-version 1 > "$TMP_JSON"

python3 - "$TMP_JSON" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as f:
    data = json.load(f)

# Build {package_name -> [non-dev deps]} from cargo metadata.
edges = {}
for pkg in data.get("packages", []):
    name = pkg["name"]
    non_dev_deps = sorted({
        d["name"]
        for d in pkg.get("dependencies", [])
        if d.get("kind") not in ("dev",)
    })
    edges[name] = non_dev_deps

# Internal-crate detection: prefix-based, matching how the existing
# verify_dependencies.sh classifies crates.
INTERNAL_PREFIXES = (
    "aios-", "lago-", "arcan-", "autonomic-", "praxis-", "haima-",
    "nous-", "anima-", "ergon", "opsis-", "soma", "vigil", "spaces",
    "life-", "chronos-", "lifed", "lifegw",
)

def is_internal(dep):
    return any(dep == p.rstrip("-") or dep.startswith(p) for p in INTERNAL_PREFIXES)

# Allowed internal dependencies per source crate.
# `life-stream-metrics` is a substrate observability primitive (opentelemetry
# only — no Life impl crates, no aios-protocol coupling), added to chronos-core
# for BRO-1322 WakeRouter drain-rate metrics. It does NOT loosen the "no arcan/
# lago/autonomic impl deps in chronos-core" rule the M0 plan locked.
ALLOWED = {
    "chronos-core":     {"aios-protocol", "life-stream-metrics"},
    "chronos-triggers": {"chronos-core"},
    "chronos-lago":     {"chronos-core", "aios-protocol", "lago-core"},
    "chronos-api":      {"chronos-core"},
    "chronosd":         {
        "chronos-core", "chronos-triggers", "chronos-lago", "chronos-api",
        "aios-protocol", "lago-core", "lago-journal",
    },
}

failures = []
missing = []
for src, allowed_set in ALLOWED.items():
    if src not in edges:
        missing.append(src)
        continue
    for dep in edges[src]:
        if not is_internal(dep):
            continue
        if dep in allowed_set:
            continue
        failures.append(
            f"{src} -> {dep} is forbidden "
            f"(allowed internal deps for {src}: {sorted(allowed_set)})"
        )

if missing:
    print(f"chronos dependency audit warning: crates not in cargo metadata: {missing}")
    # Treat as failure — running the script means the workspace should know about them.
    print("chronos dependency audit FAILED (missing crates)")
    sys.exit(1)

if failures:
    print("chronos dependency audit FAILED:")
    for f in sorted(set(failures)):
        print(f"  - {f}")
    sys.exit(1)

print("chronos dependency audit passed")
PY
