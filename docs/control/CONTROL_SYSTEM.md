# Control System Model

## Purpose

Use this document to keep the Spaces repository's autonomous development loop explicit and stable. The system governs how agents and humans collaborate to evolve the SpacetimeDB module and CLI client.

## System Definition

- Setpoint: All harness gates pass (smoke, check, test), module publishes cleanly, CLI connects and round-trips messages
- Plant: The Spaces codebase — SpacetimeDB WASM module (`spacetimedb/src/`), CLI client (`src/main.rs`), generated bindings (`src/module_bindings/`), harness scripts (`scripts/`)
- Controller: Gate sequence policy (smoke -> check -> test), retry budget, escalation rules
- Actuators: Code edits, binding regeneration, module republish, harness/doc updates
- Sensors: CI results, `spacetime logs`, compile times, audit scores, client connection health
- Feedback channels: CI output, `spacetime logs spaces`, harness audit reports, terminal test output
- Disturbances: SpacetimeDB SDK version bumps, maincloud outages, schema migrations, Rust toolchain updates

## Maturity Targets

- Stability target: All gates green for 5 consecutive commits
- Adaptation target: New reducer added and tested within one development cycle (smoke -> publish -> test)
- Recovery target: Schema-breaking change recovered (rebuild bindings, fix client) within 30 minutes

## Review Cadence

- Weekly harness review owner: project maintainer — verify audit scores, review CI durations
- Monthly architecture review owner: project maintainer — assess table schema growth, reducer complexity, client UX
- Entropy cleanup cadence: bi-weekly — remove unused tables/reducers, update stale docs, trim dead bindings
