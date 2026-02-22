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

python3 - "$TMP_DIR/aios.json" "$TMP_DIR/arcan.json" "$TMP_DIR/lago.json" <<'PY'
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

if failures:
    print("architecture dependency audit failed:")
    for failure in sorted(set(failures)):
        print(f"  - {failure}")
    sys.exit(1)

print("architecture dependency audit passed")
PY
