//! Smoke test: InProcessInferenceBackend speaks the trait shape and
//! emits at least one `Token::Done` before closing. The actual model
//! call is mocked via the test fixture below — we are not exercising
//! arcan-core's real network path here.

use std::time::Duration;

use futures::StreamExt;
use inference_core::{
    AnimaIdRef, FinishReason, InMemoryKvCache, InferenceBackend, ModelId, StepContext, Token,
};

#[tokio::test]
async fn inprocess_emits_done_token() {
    let backend =
        inference_core::InProcessInferenceBackend::new_for_test(vec!["fake-model"]);
    let cache = InMemoryKvCache::new();
    let anima = AnimaIdRef::new("did:key:zDn-test");
    let kv_root = cache.allocate_for_test();

    let ctx = StepContext {
        model: ModelId::new("fake-model"),
        anima,
        kv: cache.as_ref(),
        kv_root,
        prompt_tokens: b"hello",
        max_new_tokens: 4,
        deadline: Some(std::time::Instant::now() + Duration::from_secs(5)),
        from_token: None,
        with_tool_result: None,
    };

    let mut stream = backend.step(ctx);
    let mut got_done = false;
    while let Some(item) = stream.next().await {
        let token = item.expect("no error");
        if let Token::Done { reason, .. } = token {
            assert_eq!(reason, FinishReason::Stop);
            got_done = true;
        }
    }
    assert!(got_done, "stream must emit Token::Done");
}

#[test]
fn inprocess_advertises_capabilities() {
    let backend =
        inference_core::InProcessInferenceBackend::new_for_test(vec!["fake-model"]);
    let caps = backend.capabilities();
    assert!(!caps.spec_decode, "in-process wraps aisdk; no spec decode");
    assert!(!caps.fast_swap);
    assert_eq!(backend.backend_id(), "in-process");
}
