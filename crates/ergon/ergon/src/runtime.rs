//! Runtime extension points — traits ergon expects the host runtime to
//! implement.
//!
//! These three traits are the **seam between ergon (Layer 2) and the
//! runtime it executes inside**. Today the runtime is `arcan` (Layer 3)
//! and the implementations are wired up by the arcan adapter
//! (BRO-1001 — `core/life/crates/arcan/arcan/src/agent_kind/ergon.rs`).
//! Future runtimes can implement the same three traits without exposing
//! arcan internals — that's the whole point of ergon owning these
//! abstractions.
//!
//! ## Why ergon owns these traits (not `arcan_provider` / `praxis_core`)
//!
//! The spec (§3.4) listed these as direct dependencies on
//! `arcan_provider::Provider` and `praxis_core::ToolRegistry`. We
//! deliberately deviate:
//!
//! 1. **Hook signatures depend on these.** Every `Hook::on_pre_tool_use`
//!    receives a `&mut ToolCall` whose dispatch goes through `ToolRegistry`.
//!    If `ToolRegistry` lived in `praxis_core`, every change in praxis
//!    would ripple through every hook implementation.
//! 2. **Mockability.** Tests against ergon's loop need a `Provider` mock.
//!    Mocking `arcan_provider::Provider` requires importing all of
//!    arcan-provider; mocking `ergon::Provider` requires nothing.
//! 3. **Substrate independence.** Ergon's contract should be testable
//!    without any substrate present. Today this PR validates that — no
//!    arcan / praxis / lago / vigil dependencies are pulled in.
//!
//! See `crates/ergon/ergon/CLAUDE.md` for the full rationale.

use crate::error::Result;
use crate::model::{ModelRequest, ModelResponse, ToolCall, ToolDefinition, ToolResult};
use crate::stream::StreamSink;
use async_trait::async_trait;
use std::sync::Arc;

/// Streaming model provider.
///
/// A `Provider` is responsible for taking a [`ModelRequest`], invoking the
/// underlying LLM API (Anthropic, OpenAI, Bedrock, etc.), and:
/// 1. **Pushing** every observable event to the supplied [`StreamSink`]
///    as it arrives — text deltas, reasoning, tool-use starts/ends, usage,
///    citations, etc.
/// 2. **Returning** the assembled [`ModelResponse`] when the stream
///    terminates (with the final stop_reason and concatenated content).
///
/// ## Push semantics
///
/// The sink is the primary observability channel — anything that wants to
/// see the stream as it happens (durable replay via `LagoSink`, OTel via
/// `VigilSink`, user-facing SSE via `LifegwSink`) hooks into it. The
/// returned `ModelResponse` is the *post-stream summary* used by the
/// autonomous loop to decide whether to dispatch tools or break.
///
/// ## Backpressure
///
/// `sink.emit()` is async and may block on a slow consumer. The provider
/// MUST respect that backpressure (await every emit) and propagate
/// cancellation if the upstream stream is dropped.
///
/// ## Implementations
///
/// In production: an `ArcanProviderAdapter` (BRO-1001) wraps
/// `arcan_provider::Provider` and translates its events to ergon's
/// [`crate::StreamEvent`] taxonomy. In tests: see the mock impl in
/// `step.rs` `tests` module.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Stable, human-readable name of the provider (used in tracing,
    /// `StreamEvent::SessionStart`, and error messages). Convention:
    /// kebab-case, e.g. `"anthropic"`, `"openai"`, `"mock"`.
    fn name(&self) -> &str;

    /// Stream a single turn of model output.
    ///
    /// **Contract**:
    /// - Every observable event is forwarded to `sink` in emission order
    ///   (or an error from the sink terminates the stream).
    /// - The returned [`ModelResponse`] is the post-stream summary —
    ///   the same content blocks that were streamed, plus the final
    ///   `stop_reason` and `Usage` totals.
    /// - On provider error, returns [`crate::ErgonError::Provider`].
    /// - On sink error (e.g., consumer disconnect), returns
    ///   [`crate::ErgonError::StreamClosed`].
    async fn stream(&self, req: ModelRequest, sink: Arc<dyn StreamSink>) -> Result<ModelResponse>;
}

/// Registry of tools the autonomous loop can dispatch on the model's behalf.
///
/// A `ToolRegistry` owns:
/// - The list of [`ToolDefinition`]s exposed to the model in each turn
///   (used to populate [`ModelRequest::tools`]).
/// - The dispatch logic mapping a [`ToolCall`] to a [`ToolResult`].
///
/// ## Why sandbox policy is internal to the registry
///
/// The spec listed `sandbox: Arc<praxis_core::SandboxPolicy>` as a separate
/// field on `StepCtx`. We folded that into the `ToolRegistry` impl: the
/// sandbox is bound at registry construction time, not passed per-call.
/// This keeps `step.rs` substrate-free and lets registries with different
/// security models (sandboxed praxis vs. trusted in-process tools)
/// implement the same trait.
///
/// ## Implementations
///
/// In production: a `PraxisToolRegistryAdapter` (BRO-1001) wraps
/// `praxis_core::ToolRegistry` and bakes a `SandboxPolicy` in at
/// construction. In tests: see the mock impl in `step.rs` `tests` module.
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// JSON-Schema-backed tool definitions advertised to the model.
    ///
    /// The autonomous loop calls this **once per turn** (well, once per
    /// `run_inference_streaming` invocation) and inserts the result into
    /// [`ModelRequest::tools`]. Hooks can then mutate the list in
    /// `on_pre_inference` (e.g., narrow per skill).
    fn definitions(&self) -> Vec<ToolDefinition>;

    /// Dispatch a single tool invocation.
    ///
    /// Returns a [`ToolResult`] carrying either the tool's output or a
    /// model-visible error (`is_error = true`) the model can reason about.
    /// Hard runtime failures (sandbox panic, capability denial outside the
    /// registry's purview) surface as [`crate::ErgonError::Tool`].
    async fn invoke(&self, call: ToolCall) -> Result<ToolResult>;
}

/// Narrow back-channel from a workflow body into the host runtime.
///
/// `RuntimeHandle` is the **controlled escape hatch** ergon exposes for
/// primitives it doesn't itself abstract. v0.1 keeps the surface minimal
/// (only [`Self::operating_mode`]); future versions may add
/// `aios_caps()`, `edit_hashline()`, and similar — but each new method is
/// a deliberate boundary expansion, not a default.
///
/// ## Why narrow?
///
/// The spec proposed exposing all of `arcan_core::TickHandle` through this
/// trait. That's the wrong direction — it would couple every workflow to
/// arcan's full internal API surface. Per spec §8 Q2, we narrow to
/// "exactly the methods ergon-running workflows should call." For v0.1,
/// that's just operating-mode introspection.
///
/// ## Implementations
///
/// Today: arcan's `TickCtx` implements this directly (BRO-1001). Tests:
/// see the mock impl in `step.rs` `tests` module.
pub trait RuntimeHandle: Send + Sync {
    /// Current operating mode of the host runtime's FSM.
    ///
    /// Workflows can branch on this — for example, taking a more
    /// conservative action when in [`OperatingMode::Recover`].
    fn operating_mode(&self) -> aios_protocol::mode::OperatingMode;
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_protocol::mode::OperatingMode;

    /// Trivial RuntimeHandle for compile-time API checks.
    struct StaticRuntime(OperatingMode);

    impl RuntimeHandle for StaticRuntime {
        fn operating_mode(&self) -> OperatingMode {
            self.0
        }
    }

    #[test]
    fn runtime_handle_is_dyn_compatible() {
        let h: Arc<dyn RuntimeHandle> = Arc::new(StaticRuntime(OperatingMode::Execute));
        assert_eq!(h.operating_mode(), OperatingMode::Execute);
    }

    #[test]
    fn runtime_handle_can_observe_recovery_mode() {
        let h = StaticRuntime(OperatingMode::Recover);
        assert_eq!(h.operating_mode(), OperatingMode::Recover);
    }
}
