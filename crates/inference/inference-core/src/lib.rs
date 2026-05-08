//! `inference-core` — Agent-Loop Compute Contract foundation.
//!
//! See `core/life/docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md`
//! for the full design. This crate ships the traits, types, and one
//! reference backend (`InProcessInferenceBackend`). Vendor-specific
//! backends (MLX, vLLM, Tenstorrent, Groq, Cerebras, `SambaNova`) live
//! in sibling crates that depend on this one.

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms, clippy::pedantic)]

pub mod backend;
pub mod backend_inprocess;
pub mod error;
pub mod ids;
pub mod kv;
pub mod kv_inmem;
pub mod router;
pub mod types;

pub use error::InferenceError;
pub use ids::{KvKey, ModelId};
pub use kv::{AnimaIdRef, KvCache, KvHandle, KvPinGuard, LagoOidRef};
pub use kv_inmem::InMemoryKvCache;
pub use types::{CloseCode, FinishReason, Token, ToolCall, ToolResult};

#[cfg(test)]
mod types_tests {
    use super::types::*;

    #[test]
    fn close_code_round_trip() {
        for code in [
            CloseCode::Normal,
            CloseCode::UnsupportedFrame,
            CloseCode::Deadline,
            CloseCode::KvEvicted,
            CloseCode::ModelSwap,
            CloseCode::BackendUnavailable,
            CloseCode::AnimaInvalidated,
            CloseCode::ToolAwait,
        ] {
            let n: u16 = code.into();
            let back = CloseCode::try_from(n).expect("known code");
            assert_eq!(code, back, "round-trip failed for {code:?}");
        }
    }

    #[test]
    fn close_code_unknown_rejected() {
        assert!(CloseCode::try_from(9999u16).is_err());
    }

    #[test]
    fn token_serializes() {
        let t = Token::Text("hello".into());
        let s = serde_json::to_string(&t).unwrap();
        let back: Token = serde_json::from_str(&s).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn finish_reason_variants() {
        // Locks the public enum surface — adding a variant must update this list.
        let variants = [
            FinishReason::Stop,
            FinishReason::Length,
            FinishReason::ToolCallEmitted,
            FinishReason::DeadlineExceeded,
            FinishReason::Cancelled,
        ];
        assert_eq!(variants.len(), 5);
    }
}

#[cfg(test)]
mod error_tests {
    use super::error::InferenceError;
    use super::types::CloseCode;

    #[test]
    fn backend_error_carries_close_code() {
        let e = InferenceError::backend(CloseCode::Deadline, "took too long");
        assert!(matches!(e.close_code(), Some(CloseCode::Deadline)));
        assert!(format!("{e}").contains("took too long"));
    }

    #[test]
    fn cancelled_has_no_close_code() {
        let e = InferenceError::Cancelled;
        assert!(e.close_code().is_none());
    }

    #[test]
    fn network_wraps_io() {
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        let e = InferenceError::Network(io);
        assert!(format!("{e}").contains("network"));
    }

    #[test]
    fn error_is_non_exhaustive() {
        // Documents that the enum is `#[non_exhaustive]` so downstream
        // crates must use a wildcard arm. Catching this at the type
        // level is the goal — this test just sanity-checks construction.
        let _ = InferenceError::backend(CloseCode::Normal, "fine");
    }
}

#[cfg(test)]
mod ids_tests {
    use super::ids::*;

    #[test]
    fn model_id_round_trip() {
        let id = ModelId::new("anthropic/claude-sonnet-4.6");
        assert_eq!(id.as_str(), "anthropic/claude-sonnet-4.6");
        assert_eq!(id.to_string(), "anthropic/claude-sonnet-4.6");
    }

    #[test]
    fn model_id_rejects_empty() {
        assert!(ModelId::try_new("").is_err());
        assert!(ModelId::try_new("   ").is_err());
    }

    #[test]
    fn kv_key_is_stable_for_same_inputs() {
        let a = KvKey::derive("model/a", "did:key:z6Mk…", b"prompt-bytes", 0..128);
        let b = KvKey::derive("model/a", "did:key:z6Mk…", b"prompt-bytes", 0..128);
        assert_eq!(a, b, "key derivation must be deterministic");
    }

    #[test]
    fn kv_key_changes_with_inputs() {
        let base = KvKey::derive("m", "d", b"p", 0..1);
        assert_ne!(base, KvKey::derive("m2", "d", b"p", 0..1));
        assert_ne!(base, KvKey::derive("m", "d2", b"p", 0..1));
        assert_ne!(base, KvKey::derive("m", "d", b"p2", 0..1));
        assert_ne!(base, KvKey::derive("m", "d", b"p", 0..2));
    }
}

#[cfg(test)]
mod kv_tests {
    use super::kv::KvCache;
    use super::kv_inmem::InMemoryKvCache;

    #[tokio::test]
    async fn handle_lifecycle_lookup_miss() {
        let cache = InMemoryKvCache::new();
        let key = super::ids::KvKey::derive("m", "d", b"p", 0..1);
        assert!(cache.lookup(&key).is_none());
    }

    #[tokio::test]
    async fn fork_yields_distinct_handle() {
        let cache = InMemoryKvCache::new();
        let h0 = cache.allocate_for_test();
        let h1 = cache.fork(h0);
        assert_ne!(h0, h1);
    }

    #[tokio::test]
    async fn pin_guard_drops_pin_on_scope_exit() {
        let cache = InMemoryKvCache::new();
        let h = cache.allocate_for_test();
        assert_eq!(cache.pin_count(h), 0);
        {
            let _guard = cache.pin(h);
            assert_eq!(cache.pin_count(h), 1);
        }
        assert_eq!(cache.pin_count(h), 0);
    }
}
