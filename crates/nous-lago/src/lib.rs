//! Lago persistence bridge for Nous evaluation events.
//!
//! Follows the same pattern as `autonomic-lago`:
//! - Publisher: writes `eval.*` events to the Lago journal
//! - Subscriber: reads `eval.*` events for replay and projection

pub mod publisher;
pub mod subscriber;

pub use publisher::NousPublisher;
pub use subscriber::NousSubscriber;
