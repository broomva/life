# Sensors

List the signals used to evaluate whether the Spaces system is on target.

## Required Sensors

- CI results: smoke (WASM compile + client compile), check (clippy --all-targets), test (cargo build + publish)
- SpacetimeDB server logs: reducer execution traces, error counts, init verification
- Module publish status: success/failure from `spacetime publish`
- Client connection health: WebSocket connect, subscription applied callback, message round-trip
- Harness audit score: 21 checks (files exist + content sections)
- Control audit score: 15 checks (files exist + content sections)
- Compile times: WASM module and CLI client build durations

## Signal Contracts

| Sensor | Required Fields | Sampling | Storage |
|---|---|---|---|
| Harness CI | step_name, status, duration_ms, run_id | Every push | GitHub Actions logs |
| SpacetimeDB logs | timestamp, reducer_name, status, identity | Always | `spacetime logs spaces` |
| Module publish | status, duration, error_message | Every publish | CI output |
| Client connection | connected, subscription_applied, identity | Every connect | Terminal stdout |
| Compile times | target (wasm/native), duration_ms | Every smoke | CI artifacts |
| Audit results | check_name, pass_fail, total_score | Every CI run | CI output |

## Sensor Gaps

- Missing signals: Reducer execution time (not exposed by SpacetimeDB logs in detail), per-table row counts, WebSocket message latency
- Noisy/unreliable signals: First compile after dependency cache miss is much slower than incremental
- Planned remediation: Use `spacetime logs` filtering for reducer-level tracing, add timing instrumentation to harness scripts
