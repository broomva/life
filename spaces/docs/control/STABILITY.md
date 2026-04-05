# Stability

Track whether the Spaces development loop remains stable under normal and disturbed conditions.

## Stability Indicators

- Smoke pass consistency: 5+ consecutive green commits indicates stable baseline
- Compile time variance: <20% deviation from setpoint targets (60s WASM, 30s client)
- Bounded retry counts: No gate requires >1 retry in normal operation
- Reducer error rate trend: Stable or decreasing over rolling 7-day window
- Audit score: Consistently 100% on both harness (21/21) and control (15/15) checks

## Disturbance Scenarios

| Scenario | Expected Behavior | Recovery Target |
|---|---|---|
| SpacetimeDB SDK version bump | Possible compile failures in module or client | Recover within 1 day — update Cargo.toml, fix API changes, regenerate bindings |
| Rust toolchain update | Possible clippy warnings or compile errors | Recover within 4 hours — fix warnings, update CI |
| Schema migration (add table/column) | Client bindings stale, publish may fail | Recover within 30 min — regenerate bindings, rebuild client |
| Schema migration (remove/rename) | Requires `--clear-database`, data loss | Recover within 1 hour — backup consideration, republish, verify init |
| Maincloud outage | Publish and client connection fail | Recover when infra restored — retry publish, reconnect client |
| Large feature branch merge | Multiple file changes, potential conflicts | Recover within 1 day — resolve conflicts, run full CI |
| wasm32-unknown-unknown target missing | Smoke fails at WASM compile step | Recover within 10 min — `rustup target add wasm32-unknown-unknown` |

## Stabilization Playbook

1. Reconfirm setpoints — are current targets still realistic given recent changes?
2. Reduce surface area — pause feature work, focus on making all gates green
3. Enforce stricter checks — run `make ci` locally before every push
4. Run entropy cleanup — remove unused tables/reducers, update stale docs, trim dead code
5. Verify recovery — run full CI 3 times consecutively to confirm stability
