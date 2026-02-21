#!/usr/bin/env bash
set -euo pipefail

ROOT="/Users/broomva/broomva.tech/live"

echo "[conformance] protocol checks"
(cd "$ROOT/aiOS/crates/aios-protocol" && cargo test)

echo "[conformance] arcan api + stream/state sync checks"
(cd "$ROOT/arcan" && cargo test -p arcand --test sse_server)

echo "[conformance] arcan-lago replay/bridge checks"
(cd "$ROOT/arcan" && cargo test -p arcan-lago --test end_to_end)

echo "[conformance] lago journal sequence assignment checks"
(cd "$ROOT/lago" && cargo test -p lago-journal append_ignores_caller_provided_seq)

echo "[conformance] lago api sse/session checks"
(cd "$ROOT/lago" && cargo test -p lago-api --test e2e_sessions)

echo "[conformance] OK"
