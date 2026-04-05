# Setpoints

Define numeric targets for the Spaces autonomous development loop.

## Core Setpoints

| Metric | Target | Alert Threshold | Owner |
|---|---|---|---|
| PR pass@1 | 90% | <80% | CI / maintainer |
| Time to actionable failure | <30s | >60s | Harness scripts |
| Merge cycle time | <30 min | >1 hour | CI pipeline |
| Revert rate | <5% | >10% | Maintainer |
| Human intervention rate | <20% | >30% | Agent workflow |
| WASM compile time | <60s | >90s | `scripts/harness/smoke.sh` |
| Client compile time | <30s | >60s | `scripts/harness/smoke.sh` |
| Module publish time | <2 min | >5 min | `spacetime publish` |
| Reducer error rate | <2% | >5% | `spacetime logs spaces` |

## Constraints

- Required quality gates: smoke (WASM + client compile), check (clippy), test (cargo build + optional publish verification)
- Security constraints: All authorization server-side via ctx.sender(), role hierarchy enforced in auth.rs, no client-trusted identity
- Cost/runtime constraints: Maincloud publish is free, WASM sandbox limits apply (deterministic reducers, no network/filesystem)
