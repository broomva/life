//! Error types and Anthropic-format error event serialization.
//!
//! Anthropic Messages encodes upstream / mid-stream errors inside the
//! SSE body itself (`event: error\ndata: {...}`), not via HTTP status
//! codes — once a 200-OK SSE response has started, every subsequent
//! signal must travel as an SSE event. This module owns:
//!
//! 1. [`CodecError`] — the typed error returned by codec entry points
//!    (request validation, sid synthesis, encoder construction). These
//!    fail *before* SSE body starts and so they SHOULD turn into HTTP
//!    4xx/5xx by the lifegw handler.
//!
//! 2. [`AnthropicError`] — the in-stream error event payload. Maps
//!    upstream `EventKind::Error` and codec mid-stream faults into the
//!    `{type: "error", error: {type, message}}` shape Claude Code
//!    expects.
//!
//! 3. [`AnthropicErrorKind`] — the closed set of error type strings
//!    Anthropic publishes
//!    (<https://docs.anthropic.com/en/api/errors>).

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Anthropic-vocabulary error categorisation.
///
/// Mirrors the public list at
/// <https://docs.anthropic.com/en/api/errors>. The `Other` variant
/// keeps forward compatibility — Claude Code tolerates unknown
/// `error.type` values by surfacing the `message` field anyway.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AnthropicErrorKind {
    /// The request was malformed (`invalid_request_error`).
    InvalidRequestError,
    /// Authentication or authorization failed (`authentication_error`).
    AuthenticationError,
    /// The caller does not have permission (`permission_error`).
    PermissionError,
    /// Requested entity does not exist (`not_found_error`).
    NotFoundError,
    /// Per-account or per-organization rate limit hit (`rate_limit_error`).
    RateLimitError,
    /// Upstream provider is overloaded (`overloaded_error`).
    OverloadedError,
    /// Generic API-side failure (`api_error`).
    ApiError,
    /// Spec J-specific: insufficient haima credits / x402 challenge
    /// (`billing_error`). Anthropic does not publish this exact code,
    /// but Claude Code surfaces the `message` field verbatim.
    BillingError,
}

impl AnthropicErrorKind {
    /// Wire string used in the `error.type` JSON field.
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::InvalidRequestError => "invalid_request_error",
            Self::AuthenticationError => "authentication_error",
            Self::PermissionError => "permission_error",
            Self::NotFoundError => "not_found_error",
            Self::RateLimitError => "rate_limit_error",
            Self::OverloadedError => "overloaded_error",
            Self::ApiError => "api_error",
            Self::BillingError => "billing_error",
        }
    }
}

/// In-stream error event payload — `{type:"error", error:{type, message}}`.
///
/// Encoded as the `data:` body of an `event: error` SSE frame. Use
/// [`AnthropicError::to_sse_data`] to obtain the JSON string.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnthropicError {
    /// Always the literal string `"error"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The inner error object Anthropic clients introspect.
    pub error: AnthropicErrorBody,
}

/// Inner `error: {...}` object inside an Anthropic SSE error event.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnthropicErrorBody {
    /// Wire string from [`AnthropicErrorKind::as_wire_str`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Human-readable message. Surfaces verbatim in Claude Code's chat.
    pub message: String,
}

impl AnthropicError {
    /// Build an in-stream error event payload.
    pub fn new(kind: AnthropicErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind: "error".to_string(),
            error: AnthropicErrorBody {
                kind: kind.as_wire_str().to_string(),
                message: message.into(),
            },
        }
    }

    /// Serialize the payload to its `data: <...>` JSON form.
    ///
    /// # Panics
    ///
    /// Never — the structure is fully owned and contains only
    /// `String`/`&str` data which `serde_json` always serializes.
    pub fn to_sse_data(&self) -> String {
        // `serde_json::to_string` only fails on integer-overflow / custom
        // serializers; ours is plain strings, so the unwrap is sound.
        serde_json::to_string(self).expect("AnthropicError is always serializable")
    }

    /// Produce the full SSE frame `event: error\ndata: <json>\n\n`.
    pub fn to_sse_frame(&self) -> String {
        format!("event: error\ndata: {}\n\n", self.to_sse_data())
    }
}

/// Typed codec entry-point error.
///
/// Returned by request validators, sid synthesis, encoder construction,
/// and any other surface that runs *before* an SSE response body has
/// started. lifegw maps these to HTTP 4xx / 5xx; once the stream is
/// open, mid-stream faults become [`AnthropicError`] frames instead.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CodecError {
    /// The inbound request was structurally invalid. Maps to HTTP 400.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// The `anthropic-version` header value is not supported. Maps to
    /// HTTP 400 per Spec J L10-D5.
    #[error("unsupported anthropic-version: {0}")]
    UnsupportedAnthropicVersion(String),

    /// The first user message could not be located. Required for sid
    /// synthesis (Spec J L10-D2). Maps to HTTP 400.
    #[error("request contained no user message")]
    NoUserMessage,

    /// JSON encoding/decoding failure. Maps to HTTP 400 on inbound
    /// parse, HTTP 500 on outbound serialization (which should be
    /// unreachable).
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Upstream lifed `EventKind::Error` carried a body that we
    /// translated into an in-stream Anthropic error event but could not
    /// otherwise classify. Held for the encoder's call site to decide
    /// whether to keep the stream open.
    #[error("upstream error: {0}")]
    Upstream(String),
}

impl CodecError {
    /// Convert a codec entry-point error into an Anthropic SSE error
    /// frame suitable for emission once the SSE body has started.
    pub fn to_sse_frame(&self) -> String {
        let kind = match self {
            Self::InvalidRequest(_)
            | Self::NoUserMessage
            | Self::UnsupportedAnthropicVersion(_)
            | Self::Json(_) => AnthropicErrorKind::InvalidRequestError,
            Self::Upstream(_) => AnthropicErrorKind::ApiError,
        };
        AnthropicError::new(kind, self.to_string()).to_sse_frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_kind_wire_strings_are_stable() {
        // Lock the public wire vocabulary. Changing any of these is a
        // protocol break for clients that introspect `error.type`.
        assert_eq!(
            AnthropicErrorKind::InvalidRequestError.as_wire_str(),
            "invalid_request_error"
        );
        assert_eq!(
            AnthropicErrorKind::AuthenticationError.as_wire_str(),
            "authentication_error"
        );
        assert_eq!(
            AnthropicErrorKind::PermissionError.as_wire_str(),
            "permission_error"
        );
        assert_eq!(
            AnthropicErrorKind::NotFoundError.as_wire_str(),
            "not_found_error"
        );
        assert_eq!(
            AnthropicErrorKind::RateLimitError.as_wire_str(),
            "rate_limit_error"
        );
        assert_eq!(
            AnthropicErrorKind::OverloadedError.as_wire_str(),
            "overloaded_error"
        );
        assert_eq!(AnthropicErrorKind::ApiError.as_wire_str(), "api_error");
        assert_eq!(
            AnthropicErrorKind::BillingError.as_wire_str(),
            "billing_error"
        );
    }

    #[test]
    fn anthropic_error_serializes_to_canonical_shape() {
        let err = AnthropicError::new(AnthropicErrorKind::RateLimitError, "slow down");
        let data = err.to_sse_data();
        // Field order is fixed by `#[derive(Serialize)]` because serde
        // preserves struct declaration order.
        assert_eq!(
            data,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#
        );
    }

    #[test]
    fn anthropic_error_sse_frame_has_blank_line_terminator() {
        let err = AnthropicError::new(AnthropicErrorKind::ApiError, "upstream failed");
        let frame = err.to_sse_frame();
        assert!(frame.starts_with("event: error\n"));
        assert!(frame.contains("\ndata: "));
        // SSE frames must end with `\n\n` so the reader's frame parser
        // sees the boundary.
        assert!(frame.ends_with("\n\n"));
    }

    #[test]
    fn codec_error_invalid_request_renders_to_invalid_request_error() {
        let e = CodecError::InvalidRequest("missing model".into());
        let frame = e.to_sse_frame();
        assert!(frame.contains("\"type\":\"invalid_request_error\""));
        assert!(frame.contains("missing model"));
    }

    #[test]
    fn codec_error_upstream_renders_to_api_error() {
        let e = CodecError::Upstream("backend hung up".into());
        let frame = e.to_sse_frame();
        assert!(frame.contains("\"type\":\"api_error\""));
    }

    #[test]
    fn codec_error_unsupported_version_renders_to_invalid_request_error() {
        let e = CodecError::UnsupportedAnthropicVersion("2099-12-31".into());
        let frame = e.to_sse_frame();
        assert!(frame.contains("\"type\":\"invalid_request_error\""));
        assert!(frame.contains("2099-12-31"));
    }
}
