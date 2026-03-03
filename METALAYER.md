# METALAYER

This repository operates as a control loop for autonomous agent development of the Spaces communication platform on SpacetimeDB.

## Setpoints

- pass_at_1 target: 90% — PRs pass CI on first attempt
- merge_cycle_time target: <30 min — from branch push to merge
- revert_rate target: <5% — reverts as percentage of merges
- human_intervention_rate target: <20% — fraction of agent changes requiring human correction
- wasm_compile_time target: <60s — SpacetimeDB module WASM compilation
- client_build_time target: <30s — Rust CLI client compilation

## Sensors

- CI checks (smoke: WASM compile + client compile, test: full build + publish, lint: clippy)
- SpacetimeDB server logs (`spacetime logs spaces`) — reducer errors, init failures
- Module publish status — successful deployment to maincloud
- Client connection health — WebSocket connect, subscription applied, message round-trip
- Harness audit results — all 21 harness checks
- Control audit results — all 15 control checks

## Controller Policy

- Gate sequence: smoke -> check -> test
- Retry budget: 2 retries per gate before escalation
- Escalation conditions: WASM compile failure (schema change), publish failure (maincloud issue), binding generation mismatch

## Actuators

- Code edits to `spacetimedb/src/` (module schema + reducers)
- Code edits to `src/main.rs` (CLI client)
- Binding regeneration via `spacetime generate`
- Module republish via `spacetime publish`
- Harness script updates (`scripts/harness/`, `scripts/control/`)
- Documentation updates (`docs/`, `PLANS.md`, `METALAYER.md`)

## Feedback Loop

1. Measure: run `make smoke` + audits, capture pass/fail and durations
2. Compare: check against setpoints (compile time, pass rate, audit results)
3. Decide: if failing — identify whether schema, reducer, client, or infra issue
4. Act: apply minimal fix, regenerate bindings if schema changed, republish if reducer changed
5. Verify: re-run `make smoke`, confirm `spacetime logs spaces` shows no errors
