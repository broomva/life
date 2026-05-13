//! The [`WakeTrigger`] async trait.

use crate::WakeEvent;

/// A source of wake events. Implementations push events into the router by returning them
/// from [`WakeTrigger::next_wake`] in a loop. The router spawns each trigger as its own
/// tokio task and forwards their events into a single stream.
///
/// ## Returning `None`
///
/// A trigger returns `None` to signal it has no more events — either because it's a
/// one-shot, or because it received an internal shutdown signal. The router then drops
/// the trigger; remaining triggers keep producing.
///
/// ## Trait object safety
///
/// `#[async_trait::async_trait]` boxes the future, which makes `Box<dyn WakeTrigger>` work.
/// The boxing overhead is negligible for the wake rate Chronos expects (≤ 100/sec system-wide).
#[async_trait::async_trait]
pub trait WakeTrigger: Send {
    /// Pull the next wake event from this trigger.
    ///
    /// Returning `None` signals the trigger is exhausted; the router will stop polling it.
    async fn next_wake(&mut self) -> Option<WakeEvent>;

    /// Short identifier for logs and observability (e.g. `"heartbeat"`, `"http"`).
    fn name(&self) -> &'static str;
}
