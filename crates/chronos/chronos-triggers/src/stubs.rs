//! Stub trigger implementations for taxonomy variants not yet implemented in M0.
//!
//! Each stub implements [`chronos_core::WakeTrigger`] but returns `None` from `next_wake`,
//! which causes the router to drop it cleanly. They exist so chronosd can be wired with the
//! full taxonomy from day one — when a real implementation arrives, swap the stub for the
//! real type at the call site without touching the router.

use async_trait::async_trait;
use chronos_core::{WakeEvent, WakeTrigger};

/// HTTP wake-trigger placeholder. Real impl in M1 backed by `chronos-api`.
pub struct HttpTriggerStub;

#[async_trait]
impl WakeTrigger for HttpTriggerStub {
    async fn next_wake(&mut self) -> Option<WakeEvent> {
        None
    }

    fn name(&self) -> &'static str {
        "http (stub)"
    }
}

/// Cron expression-driven wake-trigger placeholder. Real impl beyond M3 via `tokio-cron-scheduler`.
pub struct CronTriggerStub;

#[async_trait]
impl WakeTrigger for CronTriggerStub {
    async fn next_wake(&mut self) -> Option<WakeEvent> {
        None
    }

    fn name(&self) -> &'static str {
        "cron (stub)"
    }
}

/// Filesystem-watch wake-trigger placeholder. Real impl in M3 via the `notify` crate.
pub struct FsWatchTriggerStub;

#[async_trait]
impl WakeTrigger for FsWatchTriggerStub {
    async fn next_wake(&mut self) -> Option<WakeEvent> {
        None
    }

    fn name(&self) -> &'static str {
        "fs_watch (stub)"
    }
}

/// Sub-agent return wake-trigger placeholder. Real impl in M3 — listens for `agent.completed`
/// lago events and fires wakes on the parent session's agenda.
pub struct SubAgentReturnTriggerStub;

#[async_trait]
impl WakeTrigger for SubAgentReturnTriggerStub {
    async fn next_wake(&mut self) -> Option<WakeEvent> {
        None
    }

    fn name(&self) -> &'static str {
        "sub_agent_return (stub)"
    }
}

/// Metric-threshold wake-trigger placeholder. Real impl beyond M3 — fires when a configured
/// metric crosses a threshold (e.g. Nous score < 0.3 → wake "review" agent).
pub struct ThresholdTriggerStub;

#[async_trait]
impl WakeTrigger for ThresholdTriggerStub {
    async fn next_wake(&mut self) -> Option<WakeEvent> {
        None
    }

    fn name(&self) -> &'static str {
        "threshold (stub)"
    }
}

/// External webhook wake-trigger placeholder. Real impl beyond M3 — signature-validated
/// HTTPS endpoint exposed via `chronos-api`.
pub struct WebhookTriggerStub;

#[async_trait]
impl WakeTrigger for WebhookTriggerStub {
    async fn next_wake(&mut self) -> Option<WakeEvent> {
        None
    }

    fn name(&self) -> &'static str {
        "webhook (stub)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_stub_returns_none() {
        let stubs: Vec<Box<dyn WakeTrigger>> = vec![
            Box::new(HttpTriggerStub),
            Box::new(CronTriggerStub),
            Box::new(FsWatchTriggerStub),
            Box::new(SubAgentReturnTriggerStub),
            Box::new(ThresholdTriggerStub),
            Box::new(WebhookTriggerStub),
        ];
        for mut s in stubs {
            assert!(s.next_wake().await.is_none(), "stub {} returned Some", s.name());
        }
    }
}
