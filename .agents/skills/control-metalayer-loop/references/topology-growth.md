# Topology And Growth

This skill uses a split between product code and control-plane artifacts.

## Recommended Topology

- Product code: existing repo structure.
- Control plane:
  - `.control/` for policy, command catalog, topology, and state.
  - `docs/control/` for architecture, observability, and loop docs.
  - `scripts/control/` for deterministic command wrappers.
  - `.githooks/` for local gate enforcement.
  - `tests/e2e/web/` and `tests/e2e/cli/` for integration checks.
  - `evals/` for control metrics and drift tracking.

## Growth Path

1. Baseline: command wrappers + AGENTS/PLANS.
2. Governed: explicit policy, commands, topology, and metrics.
3. Autonomous: recovery scripts, nightly audits, entropy controls.

## Scaling Pattern

- Keep policy declarations data-driven (`yaml/json`) rather than hardcoded in prompts.
- Keep orchestration deterministic and inspectable.
- Add specialized primitives per domain, but preserve common command interface.
