# Project Context

## Snapshot (as documented on 2026-02-16)

- Version: `0.2.0`
- Status: `V1.5 (Stabilization Phase)`
- Tests: `526/526` passing
- Crates: `16` total (`7` Arcan + `9` Lago)
- Rust: Edition `2024`, MSRV `1.85`

## Workspace Layout

- `/Users/broomva/broomva.tech/live/arcan` - Runtime daemon and agent loop implementation.
- `/Users/broomva/broomva.tech/live/lago` - Event-sourced persistence substrate.
- `/Users/broomva/broomva.tech/live/aiOS` - Kernel contract reference (separate but present in workspace tree).
- `/Users/broomva/broomva.tech/live/docs` - Central architecture/status/roadmap documentation.

## System Relationship

`aiOS` defines the canonical contract.

`Arcan` implements runtime behavior:
- reconstruct session state
- call provider
- execute tools through sandbox
- stream responses

`Lago` provides durable event-sourced storage:
- append-only event journal
- content-addressed blob storage
- branching filesystem model
- policy engine and SSE adapters

`arcan-lago` is the bridge between runtime events and canonical Lago envelopes.

## Key Crates

Arcan side:
- `arcan-core`
- `arcan-harness`
- `arcan-store`
- `arcan-provider`
- `arcand`
- `arcan-lago`
- `arcan` (binary)

Lago side:
- `lago-core`
- `lago-journal`
- `lago-store`
- `lago-fs`
- `lago-ingest`
- `lago-api`
- `lago-policy`
- `lago-cli`
- `lagod`

## Known Gaps (stabilization blockers)

- Branching support is not fully exposed in Arcan (defaults to `main`).
- No OS-level sandbox isolation (soft sandbox only).
- Network isolation is declared but not enforced.
- `Mount` trait exists but is not implemented.
- No full conformance suite across aiOS/Arcan/Lago.
- aiOS unification remains a later phase effort.

## Working Assumptions

- Event history is the authoritative state source.
- Replay correctness and deterministic projections are core invariants.
- All meaningful actions should be represented as immutable events.
