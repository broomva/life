//! Lago persistence bridge for Nous evaluation events.
//!
//! Follows the same pattern as `autonomic-lago`:
//! - Publisher: writes `eval.*` events to the Lago journal
//! - Subscriber: reads `eval.*` events for replay and projection
//! - ScoreSink: bridges middleware on_score callback to Lago persistence

pub mod publisher;
pub mod score_sink;
pub mod subscriber;

pub use publisher::{LivePublisher, NousPublisher};
pub use score_sink::{ScoreSink, persist_scores};
pub use subscriber::{EvalProjection, EvalState, NousSubscriber};
