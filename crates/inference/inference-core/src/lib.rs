//! `inference-core` — Agent-Loop Compute Contract foundation.
//!
//! See `core/life/docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md`
//! for the full design. This crate ships the traits, types, and one
//! reference backend (`InProcessInferenceBackend`). Vendor-specific
//! backends (MLX, vLLM, Tenstorrent, Groq, Cerebras, SambaNova) live
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
