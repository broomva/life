//! Public token-stream types and close-code vocabulary.
//!
//! Close codes are inspired by the Spec C₃ §6.5 lifegw WebSocket vocabulary
//! but live in a *distinct numeric namespace* — the inference layer (L0/L1)
//! sits below the lifegw boundary, and the same numeric values may carry
//! different meanings here. See the [`CloseCode`] doc-comment for the
//! authoritative inference-layer assignments.

use serde::{Deserialize, Serialize};

/// One token (or token-equivalent event) emitted by an [`crate::InferenceBackend`].
///
/// Streams of `Result<Token, InferenceError>` are the primary return type
/// of [`crate::InferenceBackend::step`]. Variants other than `Text` carry
/// observability or control-plane meaning — see the spec for the full
/// vocabulary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Token {
    /// A normal text token. Plain UTF-8 — backends are expected to have
    /// already detokenised.
    Text(String),
    /// The model emitted a tool-call request. Per L5-D5 the stream
    /// closes with [`CloseCode::ToolAwait`] immediately after this.
    ToolCall(ToolCall),
    /// Observability: the speculator drafted `drafted` tokens and the
    /// target model accepted them. Followed by `drafted` `Text` tokens.
    SpecDecodeAccepted {
        /// Number of drafted tokens accepted in this round.
        drafted: u8,
    },
    /// Observability: the speculator drafted `drafted` tokens and the
    /// target model rejected them. Followed by 0 or more `Text` tokens
    /// (whatever the target model produced before re-syncing).
    SpecDecodeRejected {
        /// Number of drafted tokens rejected in this round.
        drafted: u8,
    },
    /// Stream is finished. `last_token_no` is the sequence number of the
    /// final emitted token; reconnect resumes at `last_token_no + 1`.
    Done {
        /// Why the stream stopped.
        reason: FinishReason,
        /// Sequence number of the final emitted token.
        last_token_no: u64,
    },
}

/// Why a stream finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Model emitted a stop token / EOS.
    Stop,
    /// Hit `max_new_tokens` before EOS.
    Length,
    /// Stream paused for tool dispatch (see L5-D5).
    ToolCallEmitted,
    /// `StepContext::deadline` reached.
    DeadlineExceeded,
    /// Caller cancelled the stream.
    Cancelled,
}

/// A model-emitted tool invocation. Praxis runs the tool; the host
/// re-enters the backend with `StepContext::with_tool_result` set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Caller-assigned ID; round-trips back in [`ToolResult::call_id`].
    pub call_id: String,
    /// Tool name as registered with Praxis.
    pub name: String,
    /// JSON arguments. Schema is the tool's responsibility.
    pub arguments: serde_json::Value,
}

/// Output of a Praxis-executed tool call. Fed back into a backend
/// via [`crate::StepContext::with_tool_result`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Round-tripped call ID from the originating [`ToolCall`].
    pub call_id: String,
    /// JSON content returned by the tool.
    pub content: serde_json::Value,
    /// `true` if the tool returned an error to the model. Backends may
    /// surface this as a different system message; semantics are not
    /// prescribed here.
    pub is_error: bool,
}

/// Inference-layer close codes — vocabulary inspired by Spec C₃ §6.5
/// but living in a distinct numeric namespace.
///
/// The lifegw layer (Spec C₃) emits its own close codes on its
/// WebSocket boundary; those values may overlap numerically with the
/// values here but carry *different meanings* (e.g., `4001` is
/// `RateLimit` at lifegw vs. `Deadline` here). Callers translating
/// between layers must do so via explicit mapping, not numeric reuse.
///
/// Used by [`crate::InferenceError::Backend`] and re-exported to
/// caller streams via the wire format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u16)]
pub enum CloseCode {
    /// Stream finished normally.
    Normal = 1000,
    /// Caller sent a frame the backend doesn't understand.
    UnsupportedFrame = 1003,
    /// `StepContext::deadline` reached.
    Deadline = 4001,
    /// KV cache for this session was evicted; caller must rehydrate
    /// via [`crate::KvCache::rehydrate`] and reissue.
    KvEvicted = 4002,
    /// Backend swapped models; resume after polling capabilities.
    ModelSwap = 4003,
    /// Backend lost upstream provider; router should pick another.
    BackendUnavailable = 4004,
    /// `AnimaId` bound to this stream was rotated; KV is invalidated.
    /// Caller resolves the new DID and restarts.
    AnimaInvalidated = 4005,
    /// L5-D5: model emitted a tool call. Stream closes; caller runs
    /// the tool through Praxis and reopens with `with_tool_result`.
    ToolAwait = 4010,
}

impl From<CloseCode> for u16 {
    fn from(c: CloseCode) -> u16 {
        c as u16
    }
}

impl TryFrom<u16> for CloseCode {
    type Error = UnknownCloseCode;
    fn try_from(n: u16) -> Result<Self, Self::Error> {
        Ok(match n {
            1000 => Self::Normal,
            1003 => Self::UnsupportedFrame,
            4001 => Self::Deadline,
            4002 => Self::KvEvicted,
            4003 => Self::ModelSwap,
            4004 => Self::BackendUnavailable,
            4005 => Self::AnimaInvalidated,
            4010 => Self::ToolAwait,
            _ => return Err(UnknownCloseCode(n)),
        })
    }
}

/// Returned from [`CloseCode::try_from`] when the wire code isn't in
/// the Spec E vocabulary. Callers should map to
/// [`crate::InferenceError::Backend`] with `UnsupportedFrame`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown close code: {0}")]
pub struct UnknownCloseCode(pub u16);
