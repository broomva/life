#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

collect_metadata() {
  local workspace_dir="$1"
  local output_file="$2"
  (cd "$workspace_dir" && cargo metadata --format-version 1 > "$output_file")
}

collect_metadata "$ROOT_DIR/aiOS" "$TMP_DIR/aios.json"
collect_metadata "$ROOT_DIR/arcan" "$TMP_DIR/arcan.json"
collect_metadata "$ROOT_DIR/lago" "$TMP_DIR/lago.json"
collect_metadata "$ROOT_DIR/autonomic" "$TMP_DIR/autonomic.json"
collect_metadata "$ROOT_DIR/praxis" "$TMP_DIR/praxis.json"

python3 - "$TMP_DIR/aios.json" "$TMP_DIR/arcan.json" "$TMP_DIR/lago.json" "$TMP_DIR/autonomic.json" "$TMP_DIR/praxis.json" <<'PY'
import json
import sys

metadata_files = sys.argv[1:]
packages = {}
edges = []

for path in metadata_files:
    data = json.load(open(path, "r", encoding="utf-8"))
    for package in data.get("packages", []):
        packages[package["name"]] = package

for package in packages.values():
    src = package["name"]
    for dep in package.get("dependencies", []):
        if dep.get("kind") == "dev":
            continue
        edges.append((src, dep["name"]))

def starts(name, prefix):
    return name.startswith(prefix)

def is_aios(name):
    return starts(name, "aios-")

def is_arcan(name):
    return starts(name, "arcan-") or name == "arcand"

def is_lago(name):
    return starts(name, "lago-") or name == "lagod"

def is_autonomic(name):
    return starts(name, "autonomic-") or name == "autonomicd"

def is_praxis(name):
    return starts(name, "praxis-")

failures = []

for src, dst in edges:
    # aiOS core must remain independent from Arcan/Lago implementations.
    if is_aios(src) and (is_arcan(dst) or is_lago(dst)):
        failures.append(f"{src} -> {dst} is forbidden (aiOS must not depend on Arcan/Lago)")

    # Lago core crates may consume only aios-protocol from aiOS.
    if is_lago(src) and src != "lago-aios-eventstore-adapter":
        if is_aios(dst) and dst != "aios-protocol":
            failures.append(
                f"{src} -> {dst} is forbidden (Lago core crates may only use aios-protocol)"
            )
        if is_arcan(dst):
            failures.append(f"{src} -> {dst} is forbidden (Lago must not depend on Arcan)")

    # Arcan crates: only host/adapters may consume aiOS implementation crates.
    if is_arcan(src):
        allowed_aios = {"aios-protocol"}
        if src in {"arcand", "arcan"}:
            allowed_aios = {"aios-protocol", "aios-runtime"}
        if src == "arcan-aios-adapters":
            allowed_aios = {"aios-protocol", "aios-policy", "aios-memory"}
        if is_aios(dst) and dst not in allowed_aios:
            failures.append(
                f"{src} -> {dst} is forbidden (outside allowed aiOS boundary for {src})"
            )

        # arcan-aios-adapters may depend on autonomic-core, autonomic-controller
        # for the embedded Autonomic controller (R5 Phase 2).
        if src == "arcan-aios-adapters":
            if is_autonomic(dst) and dst not in {"autonomic-core", "autonomic-controller"}:
                failures.append(
                    f"{src} -> {dst} is forbidden (adapters may only use autonomic-core/controller)"
                )
        else:
            # Other arcan crates must not depend on Autonomic directly.
            if is_autonomic(dst):
                failures.append(
                    f"{src} -> {dst} is forbidden (only arcan-aios-adapters may depend on Autonomic)"
                )

    # Autonomic crates may use aios-protocol and lago-core/lago-journal, but not Arcan.
    if is_autonomic(src):
        if is_aios(dst) and dst != "aios-protocol":
            failures.append(
                f"{src} -> {dst} is forbidden (Autonomic may only use aios-protocol)"
            )
        if is_arcan(dst):
            failures.append(f"{src} -> {dst} is forbidden (Autonomic must not depend on Arcan)")

    # Praxis crates may use aios-protocol only. Must not depend on Arcan/Lago/Autonomic.
    if is_praxis(src):
        if is_aios(dst) and dst != "aios-protocol":
            failures.append(
                f"{src} -> {dst} is forbidden (Praxis may only use aios-protocol)"
            )
        if is_arcan(dst):
            failures.append(f"{src} -> {dst} is forbidden (Praxis must not depend on Arcan)")
        if is_lago(dst):
            failures.append(f"{src} -> {dst} is forbidden (Praxis must not depend on Lago)")
        if is_autonomic(dst):
            failures.append(f"{src} -> {dst} is forbidden (Praxis must not depend on Autonomic)")

if failures:
    print("architecture dependency audit failed:")
    for failure in sorted(set(failures)):
        print(f"  - {failure}")
    sys.exit(1)

print("architecture dependency audit passed")
PY
