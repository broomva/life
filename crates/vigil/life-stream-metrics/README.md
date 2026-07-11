# life-stream-metrics

Stream-aware observability for the Life Agent OS — OpenTelemetry metrics for
`tokio` broadcast/mpsc **lag**, **drain rate**, and **buffer saturation**
(BRO-1322).

The Life runtime is event-sourced over broadcast channels. The pattern is
standard, but the lag between publisher and consumer is invisible by default:
when a slow consumer falls behind, the channel drops the oldest entries and
surfaces `RecvError::Lagged(skipped)` — handled correctly, but never measured.
This crate makes that lag a first-class metric.

## Two surfaces

| Surface | Use when | Instruments both ends? | Latency histogram? |
| --- | --- | --- | --- |
| `StreamMetrics` (call-site) | the channel's public type must stay a raw `tokio` sender/receiver, or it's an mpsc | no — you call record methods at send/recv sites | only if you thread a timestamp |
| `MeasuredSender` / `MeasuredReceiver` (wrapper) | the channel is self-contained and you can swap the type | yes — one type swap | yes — free, via an internal envelope |

## Metrics (`stream.broadcast.*`)

| Metric | Type | Labels |
| --- | --- | --- |
| `published_total` | Counter | `channel`, `event_type` |
| `consumed_total` | Counter | `channel`, `consumer_id`, `event_type` |
| `lagged_total` | Counter | `channel`, `consumer_id` |
| `skipped_messages_total` | Counter | `channel`, `consumer_id` |
| `buffer_saturation` | Gauge (0..1) | `channel` |
| `consumer_count` | Gauge | `channel` |
| `delta_latency_seconds` | Histogram | `channel`, `event_type` |

## Dependency footprint

Deliberately tiny: `opentelemetry` + `tokio` only. No OTLP export machinery
(that lives in `life-vigil`) and no `aios-protocol` coupling, so the substrate
primitives that publish streams — `lago-journal`, `aios-runtime`,
`chronos-core` — can adopt it without inheriting a heavy dependency graph.
Metrics degrade to a no-op when no meter provider is installed, so
instrumentation is unconditional (no feature flag).

## Instrumented channels

| Channel label | Crate | Surface |
| --- | --- | --- |
| `lago.journal.stream` | `lago-journal` | wrapper (`MeasuredSender`/`MeasuredReceiver`) |
| `arcan.substrate` | `aios-runtime` + arcand pump | call-site (`StreamMetrics`) |
| `chronos.wake_router` | `chronos-core` | call-site, counters only (mpsc ⇒ no lag) |

A Grafana dashboard template lives at
`crates/vigil/life-vigil/dashboards/stream-broadcast.json`.
