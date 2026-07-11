//! Stream-aware observability for the Life Agent OS.
//!
//! The Life runtime is event-sourced over `tokio::sync::broadcast` channels
//! (lago `Journal::stream`, the arcand substrate bus, Chronos wake taps). The
//! pattern is correct and standard, but the *lag* between publisher and
//! consumer is invisible by default: when a slow consumer falls behind, the
//! broadcast channel drops the oldest entries and surfaces
//! `RecvError::Lagged(skipped)` — handled correctly, but never measured.
//!
//! This crate makes that lag a first-class metric. It offers two surfaces:
//!
//! * [`StreamMetrics`] — a bundle of OTel instruments (`stream.broadcast.*`)
//!   for **call-site instrumentation**, where the channel's public type must
//!   stay a raw `tokio` sender/receiver (e.g. the aios-runtime substrate bus,
//!   whose `broadcast::Sender<EventRecord>` is load-bearing API) or where the
//!   channel is an mpsc (Chronos `WakeRouter`).
//! * [`MeasuredSender`] / [`MeasuredReceiver`] — a transparent broadcast
//!   wrapper for **self-contained channels** (e.g. lago-journal's notification
//!   stream), where a type swap instruments both ends at once and yields the
//!   publish→consume latency histogram for free.
//!
//! The crate depends only on `opentelemetry` + `tokio` — no OTLP export
//! machinery (that lives in `life-vigil`) and no `aios-protocol` — so the
//! substrate primitives that publish streams can adopt it without inheriting
//! a heavy dependency footprint. Metrics degrade to a no-op when no meter
//! provider is installed, so instrumentation is unconditional.

pub mod broadcast;
pub mod metrics;

pub use broadcast::{
    MeasuredReceiver, MeasuredSender, RecvError, SendError, TryRecvError, measured_channel,
    measured_channel_with,
};
pub use metrics::{
    ATTR_CHANNEL, ATTR_CONSUMER, ATTR_EVENT_TYPE, METER_NAME, METRIC_CONSUMED,
    METRIC_CONSUMER_COUNT, METRIC_DELTA_LATENCY, METRIC_LAGGED, METRIC_PUBLISHED,
    METRIC_SATURATION, METRIC_SKIPPED, StreamMetrics,
};
