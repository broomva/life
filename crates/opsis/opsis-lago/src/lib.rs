//! Opsis–Lago bridge: event-sourced persistence for the world state engine.
//!
//! Provides:
//! - [`OpsisEventWriter`]: Background MPSC-based event writer to Lago journal
//! - [`OpsisReplay`]: Startup replay to rebuild WorldState from journal
//! - [`event_map`]: Bidirectional OpsisEvent ↔ Lago EventEnvelope translation

pub mod event_map;
pub mod replay;
pub mod writer;

pub use replay::OpsisReplay;
pub use writer::OpsisEventWriter;
