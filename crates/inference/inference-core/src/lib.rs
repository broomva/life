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

pub use backend::{BackendCapabilities, InferenceBackend, SpeculativeStepContext, StepContext};
pub use backend_inprocess::InProcessInferenceBackend;
pub use error::InferenceError;
pub use ids::{KvKey, ModelId};
pub use kv::{AnimaIdRef, KvCache, KvHandle, KvPinGuard, LagoOidRef};
pub use kv_inmem::InMemoryKvCache;
pub use router::{InferencePolicy, InferenceRouter, RoutingHint, WorkloadClass};
pub use types::{CloseCode, FinishReason, Token, ToolCall, ToolResult};
