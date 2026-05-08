//! Reference [`InferenceBackend`] that wraps the existing
//! `arcan-core::aisdk` call site so Spec-E can ship without
//! breaking arcan. A thin shim for E-Sub-A — production paths
//! migrate to native backends in E-Sub-B onward.

use std::future::Future;
use std::pin::Pin;

use futures::stream::{self, Stream};

use crate::backend::{BackendCapabilities, InferenceBackend, StepContext};
use crate::error::InferenceError;
use crate::ids::ModelId;
use crate::types::{FinishReason, Token};

/// Wraps the existing single-path AI SDK call.
pub struct InProcessInferenceBackend {
    capabilities: BackendCapabilities,
    /// Test-mode flag — when true, `step` emits a synthetic stream
    /// without calling out. Production wiring (post-E-Sub-A) replaces
    /// the shim with a real `arcan-core::aisdk` call.
    test_mode: bool,
}

impl InProcessInferenceBackend {
    /// Construct for production use. The actual aisdk wiring is
    /// deferred to a follow-up sub-phase; today this delegates to
    /// the test path.
    #[must_use]
    pub fn new(supported: Vec<ModelId>) -> Self {
        Self {
            capabilities: BackendCapabilities::minimal()
                .with_supported_models(supported)
                .with_max_context_tokens(200_000),
            test_mode: false,
        }
    }

    /// Construct in synthetic mode for tests and CI.
    #[must_use]
    pub fn new_for_test(supported: Vec<&str>) -> Self {
        Self {
            capabilities: BackendCapabilities::minimal()
                .with_supported_models(supported.into_iter().map(ModelId::new).collect())
                .with_max_context_tokens(200_000),
            test_mode: true,
        }
    }
}

impl InferenceBackend for InProcessInferenceBackend {
    fn backend_id(&self) -> &str {
        // Static literal — the trait API binds the lifetime to &self,
        // so we explicitly cast to a `&'static str` slice via let.
        const ID: &str = "in-process";
        ID
    }

    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    fn step<'a>(
        &'a self,
        ctx: StepContext<'a>,
    ) -> Pin<Box<dyn Stream<Item = Result<Token, InferenceError>> + Send + 'a>> {
        if self.test_mode {
            // Emit `Token::Text("ok")` then `Token::Done` — enough to
            // exercise the trait shape without an external dep.
            let _ = ctx; // unused in test path
            Box::pin(stream::iter([
                Ok(Token::Text("ok".into())),
                Ok(Token::Done {
                    reason: FinishReason::Stop,
                    last_token_no: 1,
                }),
            ]))
        } else {
            // E-Sub-A wires the real aisdk path in a follow-up. For now,
            // production callers get a clear error so we don't silently
            // mis-route real traffic.
            Box::pin(stream::iter([Err(InferenceError::backend(
                crate::types::CloseCode::BackendUnavailable,
                "InProcessInferenceBackend production wiring lands in E-Sub-A follow-up; \
                 use new_for_test or a vendor backend (E-Sub-B/C onward)",
            ))]))
        }
    }

    fn swap_model<'a>(
        &'a self,
        _from: ModelId,
        _to: ModelId,
    ) -> Pin<Box<dyn Future<Output = Result<(), InferenceError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}
