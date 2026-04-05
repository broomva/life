# Controller

Describe the policy and logic that decides corrective actions for the Spaces development loop.

## Control Policy

- Primary control objective: All harness gates pass (smoke, check, test) and module publishes to maincloud without errors
- Secondary objectives: Compile times within setpoint budgets, reducer error rate below threshold, audit scores at 100%
- Priority order: (1) WASM compile, (2) client compile, (3) clippy clean, (4) publish success, (5) audit pass

## Control Inputs

- Required signals: smoke pass/fail, check pass/fail, test pass/fail, publish status, spacetime logs error count
- Input freshness constraints: CI results must be from current commit (not cached from prior run)
- Input confidence thresholds: A single smoke failure is sufficient to block; flaky signals require 2/3 failures to trigger action

## Control Actions

| Condition | Action | Scope |
|---|---|---|
| WASM compile failure | Fix schema/reducer code in `spacetimedb/src/` | Module |
| Client compile failure | Fix `src/main.rs` or regenerate bindings | Client |
| Clippy warnings | Apply suggested fixes | Module + client |
| Publish failure | Check maincloud status, verify schema compatibility | Infra |
| Binding mismatch | Run `spacetime generate`, rebuild client | Bindings |
| Audit failure | Add missing files/sections | Docs/scripts |
| Reducer error spike | Review `spacetime logs`, fix validation logic | Reducers |

## Escalation Rules

- Escalation trigger: 2 consecutive gate failures on same step, or publish failure after successful WASM compile
- Escalation owner: Project maintainer
- Maximum autonomous retries: 2 per gate step before requiring human review
- Hard escalation: Any schema migration that requires `--clear-database` (destroys production data)
