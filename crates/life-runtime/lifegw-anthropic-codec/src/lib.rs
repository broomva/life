//! `lifegw-anthropic-codec` — Anthropic Messages SSE codec for the
//! [Spec J] Phase 1 lifegw edge route.
//!
//! This crate is **edge-only** and **substrate-free**. It translates
//! lifed `pb::AgentEvent` streams into Anthropic Messages SSE chunks,
//! validates inbound Anthropic Messages requests, synthesizes
//! deterministic Life sids from `(anima DID, canonical first user
//! message)`, and tracks emitted SSE events for replay de-dup.
//!
//! The reference behaviour mirrors the MIT
//! [`Alishahryar1/free-claude-code`](https://github.com/Alishahryar1/free-claude-code)
//! Python proxy at `core/anthropic/*.py`. The Rust port reimplements
//! the mechanism; it does not embed copyrighted comments or strings
//! from the reference.
//!
//! # Locked decisions (from Spec J)
//!
//! * **L10-D1** — no substrate deps. Forbidden crates: `arcand`,
//!   `lago-*`, `haima-*`, `anima-*`, `arcan-core`, `arcan-harness`,
//!   `arcan-aios-adapters`, `inference-core`. Enforced by
//!   `scripts/verify_dependencies_lifegw_anthropic_codec.sh`.
//! * **L10-D2** — `synthesize_sid(req, did) -> Sid` returns
//!   `"claude-code:" + hex(sha256(did || "::" || canon))[..16]`.
//! * **L10-D4** — workspace-internal crate; not published to crates.io.
//! * **L10-D5** — unknown `anthropic-version` header values are
//!   rejected as `400 Bad Request`.
//! * **L10-D7** — no `tiktoken-rs`. Token counting belongs in
//!   Vigil/Haima, not in this codec.
//!
//! [Spec J]: ../../docs/superpowers/specs/2026-05-18-spec-j-claude-code-interop.md

#![deny(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

pub mod block_policy;
pub mod contracts;
pub mod encoder;
pub mod errors;
pub mod request;
pub mod sid;
pub mod state;
pub mod thinking;
pub mod tools;

// Public re-exports — the surface other crates depend on.
pub use block_policy::BlockPolicyState;
pub use encoder::{AnthropicSseEvent, Encoder, EncoderState};
pub use errors::{AnthropicError, AnthropicErrorKind, CodecError};
pub use request::{
    AnthropicMessagesRequest, AnthropicVersion, ContentBlock, Message, Role, SystemPrompt, Tool,
    ToolChoice,
};
pub use sid::{SID_PREFIX, canonicalize_first_user_message, synthesize_sid};
pub use state::EmittedTracker;
pub use thinking::ThinkingState;
pub use tools::ToolUseState;
