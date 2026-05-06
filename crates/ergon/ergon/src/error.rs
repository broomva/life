//! Error type and result alias for the ergon harness.
//!
//! Ergon is a Layer-2 trait crate; its errors are flat, structural, and never
//! leak substrate detail. Each variant maps to a category of failure visible
//! to the workflow author. Wire-level errors (transport, KMS, network)
//! surface as [`ErgonError::Provider`] or substrate-specific variants in
//! downstream impls.

use std::fmt;

/// Canonical error type for ergon workflows, steps, and hooks.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ErgonError {
    /// A workflow-level error — usually surfaced from
    /// [`crate::Workflow::execute`] (once that surface lands).
    #[error("workflow error: {0}")]
    Workflow(String),

    /// A step-level error — surfaced from `Step::run`.
    #[error("step error: {0}")]
    Step(String),

    /// A hook denied a lifecycle event (`Hook::on_*` returned `Deny(reason)`).
    #[error("hook error: {0}")]
    Hook(String),

    /// A model-provider transport error.
    #[error("provider error: {0}")]
    Provider(String),

    /// A tool invocation failed.
    #[error("tool error: {0}")]
    Tool(String),

    /// The autonomic budget gate denied inference.
    #[error("budget exhausted: {0}")]
    Budget(String),

    /// A capability check denied a tool call.
    #[error("capability denied: {0}")]
    CapabilityDenied(String),

    /// The autonomous loop exceeded its `max_turns` budget.
    #[error("max turns ({0}) exceeded")]
    MaxTurns(u32),

    /// Backpressure: the upstream stream was closed before the autonomous
    /// loop completed (e.g., client disconnect propagated to LifegwSink).
    #[error("stream closed by consumer")]
    StreamClosed,

    /// JSON serialization / deserialization failure.
    #[error("ser/de: {0}")]
    Codec(#[from] serde_json::Error),

    /// Catch-all for invariant violations or implementation bugs. Prefer a
    /// more specific variant when one fits.
    #[error("internal: {0}")]
    Internal(String),
}

/// The result type used throughout the ergon crate.
pub type Result<T> = std::result::Result<T, ErgonError>;

impl ErgonError {
    /// Build a [`ErgonError::Workflow`] from any displayable.
    pub fn workflow(msg: impl fmt::Display) -> Self {
        Self::Workflow(msg.to_string())
    }

    /// Build a [`ErgonError::Step`] from any displayable.
    pub fn step(msg: impl fmt::Display) -> Self {
        Self::Step(msg.to_string())
    }

    /// Build a [`ErgonError::Internal`] from any displayable.
    pub fn internal(msg: impl fmt::Display) -> Self {
        Self::Internal(msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_inner_message() {
        let e = ErgonError::workflow("missing input");
        assert_eq!(e.to_string(), "workflow error: missing input");
    }

    #[test]
    fn max_turns_displays_count() {
        let e = ErgonError::MaxTurns(16);
        assert_eq!(e.to_string(), "max turns (16) exceeded");
    }

    #[test]
    fn codec_from_serde_json_works() {
        let bad: std::result::Result<serde_json::Value, _> =
            serde_json::from_str("{not valid json");
        let err: ErgonError = bad.unwrap_err().into();
        assert!(matches!(err, ErgonError::Codec(_)));
    }

    #[test]
    fn stream_closed_is_distinct_variant() {
        let e = ErgonError::StreamClosed;
        assert_eq!(e.to_string(), "stream closed by consumer");
    }

    #[test]
    fn capability_denied_carries_reason() {
        let e = ErgonError::CapabilityDenied("fs_write".into());
        assert!(e.to_string().contains("fs_write"));
    }
}
