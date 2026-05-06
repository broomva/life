# CLAUDE.md — `ergon-life-sinks` crate

> Instructions for AI agents working in this crate.
> Last updated: 2026-05-06.

## What this crate is

Three Life-flavored implementations of [`ergon::StreamSink`]:

| Sink | Forwards to | Production purpose |
|---|---|---|
| `LagoSink` | `lago_core::Journal::append` (event journal) | Durable replay |
| `VigilSink` | `tracing::info!` events on the current span | OTel observability via vigil's subscriber |
| `LifegwSink` | `tokio::sync::mpsc::Sender<StreamEvent>` | User-facing SSE with backpressure |

All three implement `ergon::StreamSink` and compose via `ergon::FanoutSink`.

## Why a separate crate

Same architectural principle as `ergon-life-hooks`:

- `ergon` (the core crate) is **vendor-neutral**. Zero substrate deps.
- `ergon-life-sinks` is **Life-coupled**. Depends on `lago-core`,
  `aios-protocol`, plus the `tracing` and `tokio` ecosystem.
- A future ergon consumer with a different observability/persistence
  stack ships its own sink crate.

## Composition (production)

```rust
use ergon::{FanoutSink, StreamSink};
use ergon_life_sinks::{LagoSink, VigilSink, LifegwSink};
use std::sync::Arc;

let (lifegw_sink, lifegw_rx) = LifegwSink::with_default_capacity();
let sink: Arc<dyn StreamSink> = Arc::new(FanoutSink::new(vec![
    Arc::new(LagoSink::new(journal, session_id)),  // durable first
    Arc::new(VigilSink::new()),                    // observability
    Arc::new(lifegw_sink),                         // user-facing
]));
// hand `lifegw_rx` to lifegw for SSE encoding
```

The arcan adapter (BRO-1001) is the production caller of this composition.

## Failure semantics — three tiers

| Sink | On error | Why |
|---|---|---|
| `LagoSink` | `ErgonError::Internal` (durable replay critical) | Lost events break reconstruction |
| `VigilSink` | Always `Ok(())` (infallible) | Tracing failures shouldn't block the loop |
| `LifegwSink` | `ErgonError::StreamClosed` when consumer disconnected | Backpressure / cancellation propagates |

Order matters in `FanoutSink`: the first error short-circuits.
**Recommended order: durable (Lago) → observability (Vigil) →
user-facing (Lifegw).** That way a client-side disconnect can't lose
events from the journal.

## Dependencies (locked)

- `ergon` (this is what we implement against)
- `aios-protocol` (for `EventKind`, `SessionId`, `BranchId`, `EventId`)
- `lago-core` (for `Journal` trait + `EventEnvelope`)
- `async-trait`, `serde`, `serde_json`, `tokio` (sync features), `tracing`, `ulid`

**No** `arcan-*`, **no** `praxis-*`, **no** `anima-*`, **no**
`autonomic-*`, **no** `nous-*`, **no** `life-vigil`. Vigil's role is to
configure the global tracing subscriber; this crate emits via `tracing`
directly.

## Useful commands

```bash
cargo check -p ergon-life-sinks
cargo test  -p ergon-life-sinks --all-targets
cargo clippy -p ergon-life-sinks --all-targets -- -D warnings
cargo fmt -p ergon-life-sinks
```

## Don't

- Do not pull in `life-vigil` — `tracing` is sufficient. Vigil's
  semconv-formatted spans are not part of v0.1's scope; if needed, a
  future `VigilSinkOTel` can be added that uses semconv attributes.
- Do not add `unwrap()` / `expect()` to non-test code.
- Do not change the `event_type` constant for `LagoSink`'s Custom
  payload (`"ergon.stream"`) — replays depend on it.
- Do not reduce the failure-tier strictness of `LagoSink` (e.g., make
  it best-effort). Durable replay is critical.
