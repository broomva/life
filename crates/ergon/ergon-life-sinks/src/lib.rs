//! # ergon-life-sinks — Life-flavored stream sinks
//!
//! Three sink implementations of [`ergon::StreamSink`] that wire ergon's
//! autonomous loop into Life's observability + delivery substrate:
//!
//! | Sink | Forwards every [`ergon::StreamEvent`] to | Production purpose |
//! |---|---|---|
//! | [`LagoSink`]   | `lago_core::Journal` (event journal append) | Durable replay — every session is fully reconstructable from its event sequence |
//! | [`VigilSink`]  | `tracing` events (vigil OTel subscriber)    | OTel observability — spans, metrics, distributed tracing |
//! | [`LifegwSink`] | `tokio::sync::mpsc::Sender<StreamEvent>`     | User-facing SSE — bounded mpsc with backpressure to throttle the autonomous loop when the consumer is slow |
//!
//! ## Why a separate crate
//!
//! `ergon` (the core crate) is vendor-neutral and has zero substrate
//! dependencies. These sinks couple to Life's specific observability +
//! delivery substrate (`lago-core`, `tracing` semantics, mpsc), so they
//! live in their own sibling crate. Same architectural pattern as
//! `ergon-life-hooks` — a future ergon consumer (TS port, alternate
//! agent OS) ships its own sink set.
//!
//! ## Composition
//!
//! These sinks are typically composed via [`ergon::FanoutSink`]:
//!
//! ```ignore
//! use ergon::{FanoutSink, StreamSink};
//! use ergon_life_sinks::{LagoSink, VigilSink, LifegwSink};
//! use std::sync::Arc;
//!
//! let sink: Arc<dyn StreamSink> = Arc::new(FanoutSink::new(vec![
//!     Arc::new(LagoSink::new(journal, session_id)),
//!     Arc::new(VigilSink::new()),
//!     Arc::new(LifegwSink::new(upstream_tx)),
//! ]));
//! ```
//!
//! The arcan adapter (BRO-1001) is the place that does this composition
//! in production.
//!
//! ## Failure semantics
//!
//! | Sink | On error | Why |
//! |---|---|---|
//! | [`LagoSink`]  | Returns [`ergon::ErgonError::Internal`] | Durable replay is critical; lost events break reconstruction |
//! | [`VigilSink`] | Always `Ok(())` (never errors) | Tracing is observe-only; failures shouldn't break the loop |
//! | [`LifegwSink`] | Returns [`ergon::ErgonError::StreamClosed`] when consumer disconnected | Backpressure / cancellation propagates to the loop |
//!
//! When composed via `FanoutSink`, the first error short-circuits — so
//! ordering matters. Recommended order: durable (Lago) first, then
//! observability (Vigil), then user-facing (Lifegw). That way a
//! client-side disconnect can't lose events from the journal.

#![doc(html_no_source)]

pub mod lago;
pub mod lifegw;
pub mod vigil;

pub use lago::LagoSink;
pub use lifegw::LifegwSink;
pub use vigil::VigilSink;
