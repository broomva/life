//! Cross-backend conformance scaffold.
//!
//! E-Sub-F fans this out into per-backend test modules. For E-Sub-A
//! we run the suite against `InProcessInferenceBackend::new_for_test`
//! to lock the *contract* — a real backend that fails any of these
//! is non-conforming.
//!
//! TODO(E-Sub-F): expand this scaffold with the full backend × model ×
//! mode matrix from Spec E §"Sub-phases" → E-Sub-F:
//!   * greedy / sampled (seed-pinned) / spec-decode modes
//!   * tool-await reconnect (verify L5-D5)
//!   * KV-evict reconnect (verify L5-D2 + L5-D6)
//!   * deadline expiry
//!   * cross-backend digest equivalence (BLAKE3 over token streams)

use std::time::Duration;

use futures::StreamExt;
use inference_core::{AnimaIdRef, InMemoryKvCache, InferenceBackend, ModelId, StepContext, Token};

/// Conformance assertion: backend emits `Token::Done` before stream end.
async fn assert_emits_done<B: InferenceBackend>(backend: &B, model_str: &str) {
    let cache = InMemoryKvCache::new();
    let kv_root = cache.allocate_for_test();
    let ctx = StepContext {
        model: ModelId::new(model_str),
        anima: AnimaIdRef::new("did:key:zDn-conformance"),
        kv: cache.as_ref(),
        kv_root,
        prompt_tokens: b"conformance",
        max_new_tokens: 8,
        deadline: Some(std::time::Instant::now() + Duration::from_secs(10)),
        from_token: None,
        with_tool_result: None,
    };

    let mut stream = backend.step(ctx);
    let mut saw_done = false;
    while let Some(item) = stream.next().await {
        let tok = item.unwrap_or_else(|e| {
            panic!(
                "{}: stream must not error in conformance: {e}",
                backend.backend_id()
            )
        });
        if matches!(tok, Token::Done { .. }) {
            saw_done = true;
        }
    }
    assert!(
        saw_done,
        "{}: must emit Token::Done before closing",
        backend.backend_id()
    );
}

/// Conformance assertion: backend reports stable `backend_id`.
fn assert_stable_id<B: InferenceBackend>(backend: &B) {
    let id1 = backend.backend_id().to_owned();
    let id2 = backend.backend_id().to_owned();
    assert_eq!(id1, id2, "backend_id must be stable across calls");
    assert!(!id1.is_empty(), "backend_id must not be empty");
}

#[tokio::test]
async fn conformance_in_process_backend() {
    let backend = inference_core::InProcessInferenceBackend::new_for_test(vec!["conf-model"]);
    assert_stable_id(&backend);
    assert_emits_done(&backend, "conf-model").await;
}
