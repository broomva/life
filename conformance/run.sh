#!/usr/bin/env bash
set -euo pipefail

ROOT="/Users/broomva/broomva.tech/live"

echo "[conformance] protocol checks"
(cd "$ROOT/aiOS/crates/aios-protocol" && cargo test)

echo "[conformance] arcand canonical session API checks"
(cd "$ROOT/arcan" && cargo test -p arcand --test canonical_api)

echo "[conformance] arcan-lago replay/bridge checks"
(cd "$ROOT/arcan" && cargo test -p arcan-lago --test end_to_end)

echo "[conformance] lago journal sequence assignment checks"
(cd "$ROOT/lago" && cargo test -p lago-journal append_ignores_caller_provided_seq)

echo "[conformance] lago api sse/session checks"
(cd "$ROOT/lago" && cargo test -p lago-api --test e2e_sessions)

echo "[conformance] lago-aios-eventstore-adapter bridge checks"
(cd "$ROOT/lago" && cargo test -p lago-aios-eventstore-adapter)

echo "[conformance] golden fixture replay tests"
(cd "$ROOT/lago" && cargo test -p lago-journal --test golden_replay)

echo "[conformance] OK"
