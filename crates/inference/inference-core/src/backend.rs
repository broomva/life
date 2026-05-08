//! [`InferenceBackend`] trait — the core agent-loop contract.

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use futures::Stream;

use crate::error::InferenceError;
use crate::ids::ModelId;
use crate::kv::{AnimaIdRef, KvCache, KvHandle};
use crate::types::{Token, ToolResult};

/// Per-call inputs for [`InferenceBackend::step`].
pub struct StepContext<'a> {
    /// Which model to invoke. Backend decides whether the model is
    /// already loaded and triggers `swap_model` if not.
    pub model: ModelId,
    /// Anima identity scoping the KV cache and any audit events.
    pub anima: AnimaIdRef,
    /// Cache reference. Per L5-D2 this is typically a Lago-backed impl.
    pub kv: &'a dyn KvCache,
    /// Root of the current execution-graph branch. `KvCache::fork`
    /// when the agent loop branches.
    pub kv_root: KvHandle,
    /// Wire-form prompt prefix. Already tokenised or already encoded
    /// in whatever format the backend expects — Spec-E is opaque here.
    pub prompt_tokens: &'a [u8],
    /// Cap on emitted tokens. Backend returns
    /// [`crate::FinishReason::Length`] when hit.
    pub max_new_tokens: u32,
    /// Optional wall-clock cutoff. Backend returns
    /// [`crate::FinishReason::DeadlineExceeded`] on miss.
    pub deadline: Option<Instant>,
    /// Token sequence number to resume from (after [`crate::CloseCode::ToolAwait`]).
    pub from_token: Option<u64>,
    /// Tool result to feed back into the model after a previous
    /// [`crate::CloseCode::ToolAwait`] close. None on first call.
    pub with_tool_result: Option<ToolResult>,
}

/// Per-call inputs for [`InferenceBackend::step_speculative`].
/// Identical to [`StepContext`] plus a `draft_model` field.
pub struct SpeculativeStepContext<'a> {
    /// The target model (the slow one whose tokens count).
    pub target: ModelId,
    /// The draft model (the fast one whose tokens are checked).
    pub draft: ModelId,
    /// Maximum draft length per round-trip. Autonomic owns this.
    pub max_draft_tokens: u8,
    /// Acceptance threshold (logit overlap) below which the target
    /// rejects the draft. Backend-specific units; 0.0..=1.0 by convention.
    pub accept_threshold: f32,
    /// All other context shared with [`StepContext`].
    pub base: StepContext<'a>,
}

/// Capabilities advertised by a backend at construction time.
/// Routers and Autonomic read these to decide where to dispatch.
///
/// Note: this is a feature-flag bag, not a state machine — multiple
/// independent capabilities can be true simultaneously, so the
/// `clippy::struct_excessive_bools` heuristic does not apply.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
#[non_exhaustive]
pub struct BackendCapabilities {
    /// Backend supports speculative decoding via `step_speculative`.
    pub spec_decode: bool,
    /// Backend can switch loaded model in < 100 ms (e.g. agent-loop
    /// silicon).
    pub fast_swap: bool,
    /// KV state is persisted on-chip / in device memory across calls
    /// without spilling to host RAM.
    pub on_chip_kv_persist: bool,
    /// Model emits tool calls as a structured token (not via post-hoc
    /// parsing of plain text).
    pub native_tool_emit: bool,
    /// Hard upper bound on input + output token count.
    pub max_context_tokens: u32,
    /// Model identifiers this backend can serve.
    pub supported_models: Vec<ModelId>,
}

impl BackendCapabilities {
    /// All-false capabilities with empty model list. Backends start
    /// here and `with_*` themselves up.
    #[must_use]
    pub fn minimal() -> Self {
        Self {
            spec_decode: false,
            fast_swap: false,
            on_chip_kv_persist: false,
            native_tool_emit: false,
            max_context_tokens: 0,
            supported_models: Vec::new(),
        }
    }
    /// Set the `spec_decode` capability.
    #[must_use]
    pub fn with_spec_decode(mut self, v: bool) -> Self {
        self.spec_decode = v;
        self
    }
    /// Set the `fast_swap` capability.
    #[must_use]
    pub fn with_fast_swap(mut self, v: bool) -> Self {
        self.fast_swap = v;
        self
    }
    /// Set the `on_chip_kv_persist` capability.
    #[must_use]
    pub fn with_on_chip_kv_persist(mut self, v: bool) -> Self {
        self.on_chip_kv_persist = v;
        self
    }
    /// Set the `native_tool_emit` capability.
    #[must_use]
    pub fn with_native_tool_emit(mut self, v: bool) -> Self {
        self.native_tool_emit = v;
        self
    }
    /// Set the `max_context_tokens` capability.
    #[must_use]
    pub fn with_max_context_tokens(mut self, n: u32) -> Self {
        self.max_context_tokens = n;
        self
    }
    /// Set the list of supported models.
    #[must_use]
    pub fn with_supported_models(mut self, ms: Vec<ModelId>) -> Self {
        self.supported_models = ms;
        self
    }
}

/// The agent-loop compute contract.
///
/// All methods are on `&self` so backends can be shared across many
/// concurrent agent loops via `Arc`. Internal mutability is the
/// backend's responsibility.
pub trait InferenceBackend: Send + Sync + 'static {
    /// Stable identifier used in metrics and policy. Examples:
    /// `"mlx"`, `"vllm"`, `"groq"`, `"tt-wormhole"`.
    fn backend_id(&self) -> &str;

    /// Static capabilities. Cheap to call.
    fn capabilities(&self) -> &BackendCapabilities;

    /// Execute one model step. Returns a stream that closes with
    /// [`crate::Token::Done`] on success or
    /// [`InferenceError::Backend`] with a [`crate::CloseCode`] on failure.
    fn step<'a>(
        &'a self,
        ctx: StepContext<'a>,
    ) -> Pin<Box<dyn Stream<Item = Result<Token, InferenceError>> + Send + 'a>>;

    /// Speculative decoding. Default impl panics; backends with
    /// `capabilities().spec_decode == true` override.
    ///
    /// # Panics
    /// Default impl panics. Routers must check capabilities first.
    fn step_speculative<'a>(
        &'a self,
        _ctx: SpeculativeStepContext<'a>,
    ) -> Pin<Box<dyn Stream<Item = Result<Token, InferenceError>> + Send + 'a>> {
        panic!(
            "backend {:?} does not support speculative decoding",
            self.backend_id()
        );
    }

    /// Switch to a different model. Cost is backend-specific —
    /// agent-loop silicon advertises `fast_swap = true`.
    fn swap_model<'a>(
        &'a self,
        from: ModelId,
        to: ModelId,
    ) -> Pin<Box<dyn Future<Output = Result<(), InferenceError>> + Send + 'a>>;
}
