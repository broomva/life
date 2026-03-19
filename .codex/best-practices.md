# Best Practices

## Delivery Strategy

- Prefer vertical slices that prove end-to-end behavior.
- Integrate existing components before adding new abstractions.
- Ship changes with tests and doc updates together.

## Runtime and Persistence Patterns

- Arcan loop changes should preserve this flow:
  - reconstruct -> provider call -> tool execution -> persist -> stream
- When adding new event types, ensure:
  - forward-compatible serialization
  - mapping coverage in `arcan-lago`
  - replay projection updates
  - test coverage for round trips
- Keep tool behavior deterministic and policy-aware.
- Use hashline-safe edit patterns for file mutations.

## Lago and redb Patterns

- Treat redb as synchronous and isolate it in `spawn_blocking`.
- Keep append/read paths lightweight and predictable under load.
- Preserve compound key semantics and ordering invariants.

## Testing Patterns

- Unit tests for module logic and edge cases.
- Integration tests for crate-level flows.
- End-to-end tests for request -> events -> replay -> output lifecycle.
- Add property-style tests for key encodings, round trips, and idempotent edits where useful.

## Documentation Patterns

- Update status counts when tests or gap status change.
- Record durable troubleshooting lessons in the relevant project `CLAUDE.md`.
- Keep architecture docs aligned with real crate boundaries and execution flow.

## Security and Safety

- Default to least privilege in sandbox and policy paths.
- Never bypass policy checks for convenience.
- Avoid introducing filesystem or network behavior that weakens existing guardrails without explicit approval and tests.
