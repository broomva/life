# Conformance Harness

This directory contains the cross-stack MVP conformance runner for the hard-cutover kernel spine.

## Entry point

```bash
/Users/broomva/broomva.tech/live/conformance/run.sh
```

## What it validates

- Protocol/envelope + canonical patch model (`aios-protocol`).
- Arcand canonical API surface (`/sessions/{id}/runs|state|events|events/stream|branches|approvals`) and canonical SSE parts.
- Branch-aware repository behavior in Arcan-Lago bridge.
- Journal-assigned sequence semantics in Lago.
- SSE replay endpoint behavior in Lago API.

## Notes

- The harness intentionally reuses crate-level integration tests as acceptance probes.
- It is safe to run repeatedly; tests use temporary data stores.
