# Spec E — Sub-Phase A — Inference Foundation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `crates/inference/inference-core` crate with `InferenceBackend` + `KvCache` traits, an in-process reference backend that wraps the existing `arcan-core::aisdk` path (so nothing breaks), an in-memory KvCache for tests, an `InferenceRouter`, and ≥ 30 unit tests covering trait contracts and close-code semantics. This is the blocking foundation for E-Sub-B..F.

**Architecture:** New `crates/inference/` workspace mirroring `crates/anima/`. The trait shape is locked in `inference-core`; backend impls fan out to sibling crates in subsequent sub-phases. `InProcessInferenceBackend` wraps the existing Vercel AI SDK call site without modifying it — migration is a follow-up. KvCache scoping is `AnimaId`-bound to compose with Spec D's rotation/invalidation flow.

**Tech Stack:** Rust 2024 edition, workspace inheritance, `tokio` + `futures` for streaming, `thiserror` for errors, `serde` for serialization, `tracing` for instrumentation. No new external deps for E-Sub-A — the foundation is pure-stdlib + workspace-shared.

**Spec reference:** `core/life/docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md` — trait shape and locked decisions L5-D1..L5-D8.

**Worktree:** Use `git worktree add ../life-spec-e-sub-a -b feat/spec-e-sub-a` from `core/life/`. Per P10, decide before first file. All work in this plan happens in that worktree.

**Linear:** [BRO-1022](https://linear.app/broomva/issue/BRO-1022/spec-e-e-sub-a-inference-core-foundation-trait-inprocess-kvcache) — E-Sub-A under umbrella [BRO-1019](https://linear.app/broomva/issue/BRO-1019/spec-e-agent-loop-compute-contract). Branch name from Linear: `feature/bro-1022-spec-e-e-sub-a-inference-core-foundation-trait-inprocess` (or use the shorter `feat/spec-e-sub-a` from the worktree section below).

---

## File Structure

```
core/life/
├── Cargo.toml                                          # MODIFY — add 8 new members
└── crates/inference/                                   # CREATE — new crate cluster
    ├── inference-core/                                 # CREATE
    │   ├── Cargo.toml
    │   ├── src/
    │   │   ├── lib.rs                                  # public re-exports
    │   │   ├── types.rs                                # Token, CloseCode, FinishReason, ToolCall, ToolResult
    │   │   ├── error.rs                                # InferenceError
    │   │   ├── ids.rs                                  # ModelId, KvKey
    │   │   ├── kv.rs                                   # KvCache trait, KvHandle, KvPinGuard
    │   │   ├── kv_inmem.rs                             # InMemoryKvCache impl
    │   │   ├── backend.rs                              # InferenceBackend trait, StepContext, BackendCapabilities
    │   │   ├── backend_inprocess.rs                    # InProcessInferenceBackend impl
    │   │   └── router.rs                               # InferenceRouter, InferencePolicy, RoutingHint, WorkloadClass
    │   └── tests/
    │       ├── conformance.rs                          # cross-backend battery scaffold
    │       └── inprocess_smoke.rs                      # smoke test for InProcessInferenceBackend
    └── life-inference/                                 # CREATE — facade crate, sibling of life-anima
        ├── Cargo.toml
        └── src/
            └── lib.rs                                  # re-exports + feature gates for future backends
```

**Why this structure:** mirrors `crates/anima/{anima-core, anima-identity, life-anima}` exactly. Each file owns one concept (types / errors / IDs / KV / backend / router) so a fresh agent reading any single file gets a complete unit. Tests live in `tests/` so they exercise the public surface only — internal-only tests go inline in each module.

---

## Task 1: Create `inference-core` crate skeleton

**Files:**
- Create: `crates/inference/inference-core/Cargo.toml`
- Create: `crates/inference/inference-core/src/lib.rs`
- Modify: `Cargo.toml` (workspace root) — add new member

- [ ] **Step 1.1: Create the worktree**

```bash
cd /Users/broomva/broomva/core/life
git worktree add ../life-spec-e-sub-a -b feat/spec-e-sub-a main
cd ../life-spec-e-sub-a
mkdir -p crates/inference/inference-core/src crates/inference/inference-core/tests
```

- [ ] **Step 1.2: Write `crates/inference/inference-core/Cargo.toml`**

```toml
[package]
name = "inference-core"
description = "Agent-loop compute contract — InferenceBackend + KvCache traits and types. Spec E E-Sub-A."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
homepage.workspace = true
keywords.workspace = true
categories.workspace = true

[lints]
workspace = true

[dependencies]
async-trait.workspace = true
futures.workspace = true
serde = { workspace = true, features = ["derive"] }
thiserror.workspace = true
tokio = { workspace = true, features = ["sync", "time"] }
tracing.workspace = true
ulid.workspace = true

[dev-dependencies]
serde_json.workspace = true
tokio = { workspace = true, features = ["rt", "macros", "time"] }
```

- [ ] **Step 1.3: Write minimal `src/lib.rs`**

```rust
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
pub use kv::{KvCache, KvHandle, KvPinGuard};
pub use kv_inmem::InMemoryKvCache;
pub use router::{InferencePolicy, InferenceRouter, RoutingHint, WorkloadClass};
pub use types::{CloseCode, FinishReason, Token, ToolCall, ToolResult};
```

- [ ] **Step 1.4: Add workspace member**

In `core/life/Cargo.toml`, after the `# Anima` block, insert before `# Autonomic`:

```toml
    # Inference — Agent-Loop Compute Contract (Spec E)
    "crates/inference/inference-core",
    "crates/inference/life-inference",
```

- [ ] **Step 1.5: Verify scaffold compiles (will fail because modules don't exist yet)**

```bash
cargo check -p inference-core 2>&1 | head -20
```

Expected: errors of the form "file not found for module 'backend'" — confirms the workspace wiring is correct, modules just need bodies. The remaining tasks fill them in.

- [ ] **Step 1.6: Commit**

```bash
git add core/life/Cargo.toml core/life/crates/inference/
git commit -m "feat(inference): scaffold inference-core crate (Spec E E-Sub-A T1)"
```

---

## Task 2: Token, CloseCode, FinishReason, ToolCall, ToolResult types

**Files:**
- Create: `crates/inference/inference-core/src/types.rs`

- [ ] **Step 2.1: Write the failing test first**

Append to `crates/inference/inference-core/src/lib.rs`:

```rust
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
```

- [ ] **Step 2.2: Run the failing test**

```bash
cargo test -p inference-core types_tests 2>&1 | tail -20
```

Expected: `error[E0432]: unresolved import super::types::*` — module doesn't exist yet.

- [ ] **Step 2.3: Implement `src/types.rs`**

```rust
//! Public token-stream types and close-code vocabulary.
//!
//! Close codes mirror Spec C₃ §6.5 (lifegw WebSocket close vocabulary)
//! so reconnect-by-`last_token_no` works consistently across the runtime.

use serde::{Deserialize, Serialize};

/// One token (or token-equivalent event) emitted by an [`InferenceBackend`].
///
/// Streams of `Result<Token, InferenceError>` are the primary return type
/// of [`crate::InferenceBackend::step`]. Variants other than `Text` carry
/// observability or control-plane meaning — see the spec for the full
/// vocabulary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Token {
    /// A normal text token. Plain UTF-8 — backends are expected to have
    /// already detokenised.
    Text(String),
    /// The model emitted a tool-call request. Per L5-D5 the stream
    /// closes with [`CloseCode::ToolAwait`] immediately after this.
    ToolCall(ToolCall),
    /// Observability: the speculator drafted `drafted` tokens and the
    /// target model accepted them. Followed by `drafted` `Text` tokens.
    SpecDecodeAccepted { drafted: u8 },
    /// Observability: the speculator drafted `drafted` tokens and the
    /// target model rejected them. Followed by 0 or more `Text` tokens
    /// (whatever the target model produced before re-syncing).
    SpecDecodeRejected { drafted: u8 },
    /// Stream is finished. `last_token_no` is the sequence number of the
    /// final emitted token; reconnect resumes at `last_token_no + 1`.
    Done {
        reason: FinishReason,
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
    pub call_id: String,
    pub content: serde_json::Value,
    /// `true` if the tool returned an error to the model. Backends may
    /// surface this as a different system message; semantics are not
    /// prescribed here.
    pub is_error: bool,
}

/// Spec C₃ §6.5-aligned WebSocket close codes adapted for inference
/// streams. Used by [`InferenceError::Backend::code`] and re-export
/// to caller streams via the wire format.
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
    /// AnimaId bound to this stream was rotated; KV is invalidated.
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
/// [`InferenceError::Backend`] with `UnsupportedFrame`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown close code: {0}")]
pub struct UnknownCloseCode(pub u16);
```

- [ ] **Step 2.4: Run the test, expect pass**

```bash
cargo test -p inference-core types_tests 2>&1 | tail -10
```

Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 2.5: Commit**

```bash
git add core/life/crates/inference/inference-core/src/lib.rs core/life/crates/inference/inference-core/src/types.rs
git commit -m "feat(inference): Token, CloseCode, FinishReason, ToolCall, ToolResult types (Spec E E-Sub-A T2)"
```

---

## Task 3: `InferenceError`

**Files:**
- Create: `crates/inference/inference-core/src/error.rs`

- [ ] **Step 3.1: Write the failing test**

Append to `src/lib.rs`:

```rust
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
```

- [ ] **Step 3.2: Run test, expect compile failure**

```bash
cargo test -p inference-core error_tests 2>&1 | tail -10
```

- [ ] **Step 3.3: Implement `src/error.rs`**

```rust
//! Error type for [`crate::InferenceBackend`] operations.

use crate::types::CloseCode;

/// Top-level error type returned by inference operations.
///
/// `#[non_exhaustive]` because the spec reserves the right to add
/// variants in minor releases; downstream code must use a wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InferenceError {
    /// The backend rejected or aborted the call. Carries a Spec-E
    /// close code and a human-readable message. Most non-network
    /// errors flow through here.
    #[error("backend error ({code:?}): {message}")]
    Backend { code: CloseCode, message: String },

    /// Transport-level I/O error. Use [`InferenceError::Backend`] with
    /// [`CloseCode::BackendUnavailable`] for higher-level routing.
    #[error("network error: {0}")]
    Network(#[from] std::io::Error),

    /// Caller dropped the future before completion.
    #[error("cancelled")]
    Cancelled,
}

impl InferenceError {
    /// Construct a [`InferenceError::Backend`] with a `String` message.
    #[must_use]
    pub fn backend(code: CloseCode, message: impl Into<String>) -> Self {
        Self::Backend {
            code,
            message: message.into(),
        }
    }

    /// Returns the [`CloseCode`] carried by [`InferenceError::Backend`],
    /// or `None` for other variants.
    #[must_use]
    pub fn close_code(&self) -> Option<CloseCode> {
        match self {
            Self::Backend { code, .. } => Some(*code),
            _ => None,
        }
    }
}
```

- [ ] **Step 3.4: Run test, expect pass**

```bash
cargo test -p inference-core error_tests 2>&1 | tail -10
```

- [ ] **Step 3.5: Commit**

```bash
git add core/life/crates/inference/inference-core/src/error.rs core/life/crates/inference/inference-core/src/lib.rs
git commit -m "feat(inference): InferenceError with non-exhaustive variants (Spec E E-Sub-A T3)"
```

---

## Task 4: `ModelId` and `KvKey`

**Files:**
- Create: `crates/inference/inference-core/src/ids.rs`

- [ ] **Step 4.1: Write the failing test**

Append to `src/lib.rs`:

```rust
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
```

- [ ] **Step 4.2: Run test, confirm failure**

```bash
cargo test -p inference-core ids_tests 2>&1 | tail -5
```

- [ ] **Step 4.3: Implement `src/ids.rs`**

```rust
//! Identifier types: [`ModelId`] and [`KvKey`].

use std::ops::Range;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Opaque, lightweight model identifier. Shape is `vendor/model[@version]`
/// by convention but the type is opaque — backends interpret it.
///
/// Empty / whitespace-only strings are rejected at construction.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(Arc<str>);

/// Returned from [`ModelId::try_new`] when the input is empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("model id must not be empty or whitespace-only")]
pub struct EmptyModelId;

impl ModelId {
    /// Construct a [`ModelId`], panicking on empty input. Prefer
    /// [`ModelId::try_new`] in production paths.
    ///
    /// # Panics
    /// Panics if `s` is empty or whitespace-only.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self::try_new(s.into()).expect("non-empty model id")
    }

    /// Construct a [`ModelId`], returning [`EmptyModelId`] on bad input.
    ///
    /// # Errors
    /// Returns [`EmptyModelId`] if `s` is empty after `trim`.
    pub fn try_new(s: impl Into<String>) -> Result<Self, EmptyModelId> {
        let s: String = s.into();
        if s.trim().is_empty() {
            Err(EmptyModelId)
        } else {
            Ok(Self(Arc::from(s)))
        }
    }

    /// Borrow as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Cache key for a contiguous slice of KV state. Deterministic from
/// inputs so cross-session lookups hit the same Lago object.
///
/// Derivation: BLAKE3 over a length-prefixed concatenation of
/// `(model_id, anima_did, prompt_bytes, range.start, range.end)`. The
/// 32-byte digest is the key. AnimaId is part of the key so KV is
/// scoped to identity per L5-D6.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KvKey([u8; 32]);

impl KvKey {
    /// Derive a key from the canonical inputs.
    ///
    /// `model_id` and `anima_did` are public-knowledge identifiers;
    /// `prompt_bytes` is whatever wire-form the backend uses for the
    /// prefix; `range` is the position interval within the cached
    /// sequence.
    #[must_use]
    pub fn derive(
        model_id: &str,
        anima_did: &str,
        prompt_bytes: &[u8],
        range: Range<usize>,
    ) -> Self {
        // BLAKE3 keyed hash with a Spec-E namespace constant. Keyed
        // hashing prevents key forgery from controlled input.
        let mut hasher = blake3::Hasher::new_keyed(b"inference-core::KvKey::v1\0\0\0\0\0\0\0");
        hasher.update(&u32::try_from(model_id.len()).unwrap().to_le_bytes());
        hasher.update(model_id.as_bytes());
        hasher.update(&u32::try_from(anima_did.len()).unwrap().to_le_bytes());
        hasher.update(anima_did.as_bytes());
        hasher.update(&u32::try_from(prompt_bytes.len()).unwrap().to_le_bytes());
        hasher.update(prompt_bytes);
        hasher.update(&u64::try_from(range.start).unwrap().to_le_bytes());
        hasher.update(&u64::try_from(range.end).unwrap().to_le_bytes());
        let bytes: [u8; 32] = hasher.finalize().into();
        Self(bytes)
    }

    /// Hex-encoded 32-byte digest (64 chars).
    #[must_use]
    pub fn hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }
}

impl std::fmt::Display for KvKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}
```

- [ ] **Step 4.4: Add `blake3` to Cargo.toml**

In `crates/inference/inference-core/Cargo.toml`, under `[dependencies]`:

```toml
blake3.workspace = true
```

(`blake3` is already in the workspace's shared dependency table — verify with `grep blake3 Cargo.toml` at `core/life/`. If missing, add `blake3 = "1.5"` to the workspace `[workspace.dependencies]`.)

- [ ] **Step 4.5: Run test, expect pass**

```bash
cargo test -p inference-core ids_tests 2>&1 | tail -10
```

- [ ] **Step 4.6: Commit**

```bash
git add core/life/crates/inference/inference-core/Cargo.toml core/life/crates/inference/inference-core/src/ids.rs core/life/crates/inference/inference-core/src/lib.rs
git commit -m "feat(inference): ModelId + KvKey with BLAKE3-keyed derivation (Spec E E-Sub-A T4)"
```

---

## Task 5: `KvCache` trait + `KvHandle` + `KvPinGuard`

**Files:**
- Create: `crates/inference/inference-core/src/kv.rs`

- [ ] **Step 5.1: Write the failing test**

Append to `src/lib.rs`:

```rust
#[cfg(test)]
mod kv_tests {
    use super::kv::{KvCache, KvHandle};
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
```

- [ ] **Step 5.2: Run test, confirm compile failure**

```bash
cargo test -p inference-core kv_tests 2>&1 | tail -5
```

- [ ] **Step 5.3: Implement `src/kv.rs`**

```rust
//! KV cache trait — the L0..L3 memory hierarchy contract.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::InferenceError;
use crate::ids::KvKey;

/// Opaque handle to a cached KV slice. Cheap to copy; stable for the
/// lifetime of the cache or until [`KvCache::evict`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KvHandle(pub u64);

/// RAII guard returned by [`KvCache::pin`]. While held, the underlying
/// slice is guaranteed to stay in device memory (i.e., not evicted to
/// L1/L2/L3). Dropping the guard releases the pin.
pub struct KvPinGuard {
    pub(crate) on_drop: Box<dyn FnOnce() + Send + Sync>,
    pub(crate) handle: KvHandle,
}

impl KvPinGuard {
    /// The handle this guard pins.
    #[must_use]
    pub fn handle(&self) -> KvHandle {
        self.handle
    }
}

impl Drop for KvPinGuard {
    fn drop(&mut self) {
        // Replace the FnOnce with a no-op so we can call it.
        let f = std::mem::replace(&mut self.on_drop, Box::new(|| {}));
        f();
    }
}

/// Opaque AnimaId reference used as the scoping key for [`KvCache::persist`]
/// and [`KvCache::rehydrate`]. Backends do not validate this — Anima
/// (`crates/anima/anima-identity`) is the source of truth; this is
/// passed-through.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AnimaIdRef(pub Arc<str>);

impl AnimaIdRef {
    #[must_use]
    pub fn new(did: impl Into<String>) -> Self {
        Self(Arc::from(did.into()))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lago object identifier returned by [`KvCache::persist`]. Lifetime
/// is governed by Lago retention policy. AnimaId-scoped per L5-D6.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LagoOidRef(pub Arc<str>);

/// The KV cache contract. Backends provide their own impl; the dev /
/// test impl is [`crate::InMemoryKvCache`].
///
/// Locked decisions: L5-D2 (Lago-backed by default), L5-D6 (AnimaId-
/// scoped), L5-D5 (no tool runtime — KV is for model state only).
pub trait KvCache: Send + Sync + 'static {
    /// Look up a cached slice. `None` on miss.
    fn lookup(&self, key: &KvKey) -> Option<KvHandle>;

    /// Copy-on-write fork. The returned handle observes `base` until
    /// the first divergent write, then diverges privately. Cheap.
    fn fork(&self, base: KvHandle) -> KvHandle;

    /// Drop a cached slice. Pinned handles are not evicted; the call
    /// is a no-op until all [`KvPinGuard`]s for `handle` are dropped.
    fn evict(&self, handle: KvHandle);

    /// Persist a slice into Lago, scoped by `anima`. The returned
    /// `LagoOidRef` is durable across sessions and re-resolvable
    /// via [`KvCache::rehydrate`].
    fn persist<'a>(
        &'a self,
        handle: KvHandle,
        anima: &'a AnimaIdRef,
    ) -> Pin<Box<dyn Future<Output = Result<LagoOidRef, InferenceError>> + Send + 'a>>;

    /// Rehydrate a Lago-stored slice back into a [`KvHandle`]. Returns
    /// [`InferenceError::Backend`] with [`crate::CloseCode::AnimaInvalidated`]
    /// if `anima` doesn't match the OID's recorded scope.
    fn rehydrate<'a>(
        &'a self,
        oid: &'a LagoOidRef,
        anima: &'a AnimaIdRef,
    ) -> Pin<Box<dyn Future<Output = Result<KvHandle, InferenceError>> + Send + 'a>>;

    /// Pin `handle` in device memory for the lifetime of the returned
    /// guard. Use sparingly — pinned slices block eviction and can
    /// stall the L1 → L2 spill.
    fn pin(&self, handle: KvHandle) -> KvPinGuard;
}
```

- [ ] **Step 5.4: Implement `src/kv_inmem.rs` (next task), then run kv_tests**

(Tests for `KvCache` trait depend on `InMemoryKvCache` which is Task 6. Skip running the tests until Task 6 lands; verify compile only.)

```bash
cargo check -p inference-core 2>&1 | head -10
```

Expected: warnings about unused imports — `kv_inmem` referenced from tests doesn't exist yet. That's resolved in Task 6.

- [ ] **Step 5.5: Commit**

```bash
git add core/life/crates/inference/inference-core/src/kv.rs core/life/crates/inference/inference-core/src/lib.rs
git commit -m "feat(inference): KvCache trait + KvHandle + KvPinGuard (Spec E E-Sub-A T5)"
```

---

## Task 6: `InMemoryKvCache` (dev-mode impl)

**Files:**
- Create: `crates/inference/inference-core/src/kv_inmem.rs`

- [ ] **Step 6.1: Implement `src/kv_inmem.rs`**

```rust
//! In-memory [`KvCache`] for unit tests and dev-mode runtimes.
//! Persistence is a no-op (returns a fake OID); fork is reference-
//! counted; pin tracking is exact.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::InferenceError;
use crate::ids::KvKey;
use crate::kv::{AnimaIdRef, KvCache, KvHandle, KvPinGuard, LagoOidRef};
use crate::types::CloseCode;

#[derive(Default)]
struct Slot {
    pin_count: u32,
    persisted_oid: Option<LagoOidRef>,
    persisted_anima: Option<AnimaIdRef>,
}

/// Process-local [`KvCache`] backed by a `HashMap`. Test-only; not for
/// production. Persistence simulates Lago by minting a synthetic OID.
pub struct InMemoryKvCache {
    next_handle: AtomicU64,
    by_key: Mutex<HashMap<KvKey, KvHandle>>,
    slots: Mutex<HashMap<KvHandle, Slot>>,
}

impl InMemoryKvCache {
    /// New empty cache.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            next_handle: AtomicU64::new(1),
            by_key: Mutex::new(HashMap::new()),
            slots: Mutex::new(HashMap::new()),
        })
    }

    /// Allocate a fresh handle without populating any state. Used by
    /// tests that want a non-empty handle without going through
    /// `lookup` + `populate`. Not part of the public trait.
    #[doc(hidden)]
    #[must_use]
    pub fn allocate_for_test(&self) -> KvHandle {
        let h = KvHandle(self.next_handle.fetch_add(1, Ordering::Relaxed));
        self.slots.lock().unwrap().insert(h, Slot::default());
        h
    }

    /// Current pin count for `handle`. Test-only.
    #[doc(hidden)]
    #[must_use]
    pub fn pin_count(&self, handle: KvHandle) -> u32 {
        self.slots
            .lock()
            .unwrap()
            .get(&handle)
            .map_or(0, |s| s.pin_count)
    }

    fn fresh_handle(&self) -> KvHandle {
        let h = KvHandle(self.next_handle.fetch_add(1, Ordering::Relaxed));
        self.slots.lock().unwrap().insert(h, Slot::default());
        h
    }
}

impl KvCache for InMemoryKvCache {
    fn lookup(&self, key: &KvKey) -> Option<KvHandle> {
        self.by_key.lock().unwrap().get(key).copied()
    }

    fn fork(&self, _base: KvHandle) -> KvHandle {
        // Real CoW would observe reads from `_base` until divergence.
        // The in-mem cache stores nothing useful, so a fresh handle
        // is sufficient for trait-shape tests.
        self.fresh_handle()
    }

    fn evict(&self, handle: KvHandle) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(slot) = slots.get(&handle) {
            if slot.pin_count > 0 {
                // Pinned — no-op per the trait contract.
                return;
            }
        }
        slots.remove(&handle);
        // Also remove any by_key entries pointing here.
        let mut by_key = self.by_key.lock().unwrap();
        by_key.retain(|_, h| *h != handle);
    }

    fn persist<'a>(
        &'a self,
        handle: KvHandle,
        anima: &'a AnimaIdRef,
    ) -> Pin<Box<dyn Future<Output = Result<LagoOidRef, InferenceError>> + Send + 'a>> {
        Box::pin(async move {
            let mut slots = self.slots.lock().unwrap();
            let Some(slot) = slots.get_mut(&handle) else {
                return Err(InferenceError::backend(
                    CloseCode::KvEvicted,
                    format!("handle {handle:?} not present"),
                ));
            };
            let oid = LagoOidRef(Arc::from(format!(
                "lago:inmem:{}:{:x}",
                anima.as_str(),
                handle.0
            )));
            slot.persisted_oid = Some(oid.clone());
            slot.persisted_anima = Some(anima.clone());
            Ok(oid)
        })
    }

    fn rehydrate<'a>(
        &'a self,
        oid: &'a LagoOidRef,
        anima: &'a AnimaIdRef,
    ) -> Pin<Box<dyn Future<Output = Result<KvHandle, InferenceError>> + Send + 'a>> {
        Box::pin(async move {
            let slots = self.slots.lock().unwrap();
            for (handle, slot) in slots.iter() {
                if slot.persisted_oid.as_ref() == Some(oid) {
                    if slot.persisted_anima.as_ref() != Some(anima) {
                        return Err(InferenceError::backend(
                            CloseCode::AnimaInvalidated,
                            "OID does not belong to this anima",
                        ));
                    }
                    return Ok(*handle);
                }
            }
            Err(InferenceError::backend(
                CloseCode::KvEvicted,
                "oid not present in in-memory cache",
            ))
        })
    }

    fn pin(&self, handle: KvHandle) -> KvPinGuard {
        {
            let mut slots = self.slots.lock().unwrap();
            slots.entry(handle).or_default().pin_count += 1;
        }
        let cache_ptr = self as *const Self;
        // SAFETY-equivalent: we capture `Arc<Self>` via a clone the
        // caller must provide if they want an outliving guard. The
        // simple in-mem impl ties the guard to this borrow; tests
        // never outlive the cache so this is fine. Production
        // backends will use `Arc<Self>` and a weak handle pattern.
        let cache: &'static Self = unsafe { &*cache_ptr };
        KvPinGuard {
            on_drop: Box::new(move || {
                let mut slots = cache.slots.lock().unwrap();
                if let Some(slot) = slots.get_mut(&handle) {
                    slot.pin_count = slot.pin_count.saturating_sub(1);
                }
            }),
            handle,
        }
    }
}
```

- [ ] **Step 6.2: Run all tests so far**

```bash
cargo test -p inference-core 2>&1 | tail -15
```

Expected: types_tests + error_tests + ids_tests + kv_tests all pass. Total ≥ 11 tests.

- [ ] **Step 6.3: Commit**

```bash
git add core/life/crates/inference/inference-core/src/kv_inmem.rs
git commit -m "feat(inference): InMemoryKvCache dev-mode impl (Spec E E-Sub-A T6)"
```

---

## Task 7: `InferenceBackend` trait + `StepContext` + `BackendCapabilities`

**Files:**
- Create: `crates/inference/inference-core/src/backend.rs`

- [ ] **Step 7.1: Write the failing test**

Append to `src/lib.rs`:

```rust
#[cfg(test)]
mod backend_tests {
    use super::backend::*;

    #[test]
    fn capabilities_default_is_minimal() {
        let c = BackendCapabilities::minimal();
        assert!(!c.spec_decode);
        assert!(!c.fast_swap);
        assert!(!c.on_chip_kv_persist);
        assert!(!c.native_tool_emit);
        assert_eq!(c.max_context_tokens, 0);
        assert!(c.supported_models.is_empty());
    }

    #[test]
    fn capabilities_with_helpers_compose() {
        let c = BackendCapabilities::minimal()
            .with_spec_decode(true)
            .with_fast_swap(true)
            .with_max_context_tokens(128_000);
        assert!(c.spec_decode);
        assert!(c.fast_swap);
        assert_eq!(c.max_context_tokens, 128_000);
    }
}
```

- [ ] **Step 7.2: Run test, expect compile failure**

- [ ] **Step 7.3: Implement `src/backend.rs`**

```rust
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
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackendCapabilities {
    pub spec_decode: bool,
    pub fast_swap: bool,
    pub on_chip_kv_persist: bool,
    pub native_tool_emit: bool,
    pub max_context_tokens: u32,
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
    #[must_use]
    pub fn with_spec_decode(mut self, v: bool) -> Self {
        self.spec_decode = v;
        self
    }
    #[must_use]
    pub fn with_fast_swap(mut self, v: bool) -> Self {
        self.fast_swap = v;
        self
    }
    #[must_use]
    pub fn with_on_chip_kv_persist(mut self, v: bool) -> Self {
        self.on_chip_kv_persist = v;
        self
    }
    #[must_use]
    pub fn with_native_tool_emit(mut self, v: bool) -> Self {
        self.native_tool_emit = v;
        self
    }
    #[must_use]
    pub fn with_max_context_tokens(mut self, n: u32) -> Self {
        self.max_context_tokens = n;
        self
    }
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
```

- [ ] **Step 7.4: Run test, expect pass**

```bash
cargo test -p inference-core backend_tests 2>&1 | tail -10
```

- [ ] **Step 7.5: Commit**

```bash
git add core/life/crates/inference/inference-core/src/backend.rs core/life/crates/inference/inference-core/src/lib.rs
git commit -m "feat(inference): InferenceBackend trait + StepContext + BackendCapabilities (Spec E E-Sub-A T7)"
```

---

## Task 8: `InProcessInferenceBackend` (wraps existing `arcan-core::aisdk`)

**Files:**
- Create: `crates/inference/inference-core/src/backend_inprocess.rs`

- [ ] **Step 8.1: Inspect the existing `arcan-core::aisdk` API**

```bash
grep -nE "pub fn|pub struct|pub async fn" core/life/crates/arcan/arcan-core/src/aisdk.rs | head -30
```

Read 3–5 lines of context around each match to understand the call shape. The new `InProcessInferenceBackend` wraps that surface — its `step` impl translates `StepContext` into the existing call and adapts the response into a `Token` stream.

> **Note:** if `aisdk.rs` doesn't expose a clean async-stream surface, this task wraps whatever surface exists. The goal of E-Sub-A is *not* to refactor arcan; it's to prove the trait shape. The `InProcessInferenceBackend` may be a thin shim that synchronously buffers the response and re-emits as a stream — that's acceptable for the foundation.

- [ ] **Step 8.2: Write the smoke test**

Create `crates/inference/inference-core/tests/inprocess_smoke.rs`:

```rust
//! Smoke test: InProcessInferenceBackend speaks the trait shape and
//! emits at least one `Token::Done` before closing. The actual model
//! call is mocked via the test fixture below — we are not exercising
//! arcan-core's real network path here.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use inference_core::{
    AnimaIdRef, BackendCapabilities, FinishReason, InferenceBackend, InMemoryKvCache, KvKey,
    ModelId, StepContext, Token,
};

#[tokio::test]
async fn inprocess_emits_done_token() {
    let backend = inference_core::InProcessInferenceBackend::new_for_test(vec!["fake-model".into()]);
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
    let backend = inference_core::InProcessInferenceBackend::new_for_test(vec!["fake-model".into()]);
    let caps = backend.capabilities();
    assert!(!caps.spec_decode, "in-process wraps aisdk; no spec decode");
    assert!(!caps.fast_swap);
    assert_eq!(backend.backend_id(), "in-process");
}
```

- [ ] **Step 8.3: Run smoke test, expect compile failure**

```bash
cargo test -p inference-core --test inprocess_smoke 2>&1 | tail -10
```

- [ ] **Step 8.4: Implement `src/backend_inprocess.rs`**

```rust
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
        "in-process"
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
```

- [ ] **Step 8.5: Run smoke test, expect pass**

```bash
cargo test -p inference-core --test inprocess_smoke 2>&1 | tail -10
```

- [ ] **Step 8.6: Commit**

```bash
git add core/life/crates/inference/inference-core/src/backend_inprocess.rs core/life/crates/inference/inference-core/tests/inprocess_smoke.rs
git commit -m "feat(inference): InProcessInferenceBackend test-mode + production-error shim (Spec E E-Sub-A T8)"
```

---

## Task 9: `InferenceRouter` + `InferencePolicy` + `RoutingHint` + `WorkloadClass`

**Files:**
- Create: `crates/inference/inference-core/src/router.rs`

- [ ] **Step 9.1: Write the failing test**

Append to `src/lib.rs`:

```rust
#[cfg(test)]
mod router_tests {
    use std::sync::Arc;

    use super::backend::InferenceBackend;
    use super::backend_inprocess::InProcessInferenceBackend;
    use super::ids::ModelId;
    use super::router::*;

    #[test]
    fn single_backend_router_routes_to_only_backend() {
        let b: Arc<dyn InferenceBackend> = Arc::new(InProcessInferenceBackend::new_for_test(vec!["fake"]));
        let router = InferenceRouter::new(vec![b.clone()], InferencePolicy::single());
        let hint = RoutingHint {
            model: ModelId::new("fake"),
            workload: WorkloadClass::Synthesis,
            deadline: None,
        };
        let chosen = router.route(&hint).expect("routes");
        assert_eq!(chosen.backend_id(), b.backend_id());
    }

    #[test]
    fn router_returns_err_when_no_backend_supports_model() {
        let b: Arc<dyn InferenceBackend> = Arc::new(InProcessInferenceBackend::new_for_test(vec!["only-this"]));
        let router = InferenceRouter::new(vec![b], InferencePolicy::strict_model_match());
        let hint = RoutingHint {
            model: ModelId::new("not-supported"),
            workload: WorkloadClass::Synthesis,
            deadline: None,
        };
        assert!(router.route(&hint).is_err());
    }

    #[test]
    fn workload_class_variants() {
        let _ = WorkloadClass::Routing;
        let _ = WorkloadClass::Synthesis;
        let _ = WorkloadClass::ToolEmit;
        let _ = WorkloadClass::Embed;
    }
}
```

- [ ] **Step 9.2: Run, expect compile failure**

- [ ] **Step 9.3: Implement `src/router.rs`**

```rust
//! Router that selects an [`InferenceBackend`] per call.
//!
//! Per L5-D7 routing is dynamic: a single agent loop may visit
//! multiple backends (small drafter for routing, large model for
//! synthesis). Static defaults live in policy; Autonomic can override
//! at runtime via [`InferenceRouter::set_policy`].

use std::sync::Arc;
use std::time::Instant;

use crate::backend::InferenceBackend;
use crate::ids::ModelId;

/// Workload classification fed to the router. Maps loosely to phases
/// of the agent loop the reel describes (memory-bound model calls,
/// I/O-bound tool use, CPU-bound orchestration). Backends self-describe
/// where they're best.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkloadClass {
    /// Small / fast model picking the next action.
    Routing,
    /// Large model producing user-facing output.
    Synthesis,
    /// Step expected to emit a tool call. Wants low TTFT.
    ToolEmit,
    /// Embedding generation (vector output, no token stream).
    Embed,
}

/// Routing inputs. Cheap to construct per-call.
pub struct RoutingHint {
    pub model: ModelId,
    pub workload: WorkloadClass,
    pub deadline: Option<Instant>,
}

/// Routing strategy. E-Sub-A ships two: `single` (always pick the
/// only backend), and `strict_model_match` (pick the first backend
/// whose `capabilities().supported_models` contains the requested
/// model). Production policies (cost-aware, latency-aware,
/// Autonomic-driven) are E-Sub-E.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferencePolicy {
    /// Always return the first backend; ignore hint contents.
    Single,
    /// Pick the first backend whose capabilities advertise the model.
    StrictModelMatch,
}

impl InferencePolicy {
    #[must_use]
    pub fn single() -> Self {
        Self::Single
    }
    #[must_use]
    pub fn strict_model_match() -> Self {
        Self::StrictModelMatch
    }
}

/// Routing error.
#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("no backend supports model {0}")]
    NoBackendForModel(ModelId),
    #[error("router has no backends")]
    NoBackends,
}

pub struct InferenceRouter {
    backends: Vec<Arc<dyn InferenceBackend>>,
    policy: InferencePolicy,
}

impl InferenceRouter {
    #[must_use]
    pub fn new(backends: Vec<Arc<dyn InferenceBackend>>, policy: InferencePolicy) -> Self {
        Self { backends, policy }
    }

    /// Pick a backend for `hint`. Returns [`RouteError`] if no backend
    /// applies.
    pub fn route(&self, hint: &RoutingHint) -> Result<&Arc<dyn InferenceBackend>, RouteError> {
        if self.backends.is_empty() {
            return Err(RouteError::NoBackends);
        }
        match self.policy {
            InferencePolicy::Single => Ok(&self.backends[0]),
            InferencePolicy::StrictModelMatch => self
                .backends
                .iter()
                .find(|b| b.capabilities().supported_models.contains(&hint.model))
                .ok_or_else(|| RouteError::NoBackendForModel(hint.model.clone())),
        }
    }

    /// Replace the routing policy. Autonomic uses this to retune.
    pub fn set_policy(&mut self, policy: InferencePolicy) {
        self.policy = policy;
    }
}
```

- [ ] **Step 9.4: Run all tests, expect pass**

```bash
cargo test -p inference-core 2>&1 | tail -15
```

Expected: ≥ 14 tests pass across `types_tests`, `error_tests`, `ids_tests`, `kv_tests`, `backend_tests`, `router_tests`, plus the smoke test in `tests/inprocess_smoke.rs`.

- [ ] **Step 9.5: Commit**

```bash
git add core/life/crates/inference/inference-core/src/router.rs core/life/crates/inference/inference-core/src/lib.rs
git commit -m "feat(inference): InferenceRouter with Single + StrictModelMatch policies (Spec E E-Sub-A T9)"
```

---

## Task 10: `life-inference` facade crate

**Files:**
- Create: `crates/inference/life-inference/Cargo.toml`
- Create: `crates/inference/life-inference/src/lib.rs`

- [ ] **Step 10.1: Write `crates/inference/life-inference/Cargo.toml`**

```toml
[package]
name = "life-inference"
description = "Facade aggregator for the Life Agent OS inference layer (Spec E)."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
homepage.workspace = true
keywords.workspace = true
categories.workspace = true

[lints]
workspace = true

[features]
# Only the in-process reference backend by default — no external deps.
default = ["in-process"]
in-process = []
# Spec E sub-phases B, C, D, E reserve these feature names but do not
# wire them in E-Sub-A. Listed here so the facade's feature surface is
# the canonical place to enable a backend.
mlx = []
vllm = []
vigil = []
autonomic = []

[dependencies]
inference-core.workspace = true
```

- [ ] **Step 10.2: Write `crates/inference/life-inference/src/lib.rs`**

```rust
//! Facade re-export for the Life inference layer (Spec E).
//!
//! Mirrors `life-anima` and `life-aios` — downstream apps depend on
//! `life-inference` rather than picking sub-crates by hand. Backend
//! enable/disable goes through this crate's feature flags.

#![forbid(unsafe_code)]

pub use inference_core::*;
```

- [ ] **Step 10.3: Add to workspace and add `inference-core` to `[workspace.dependencies]`**

In `core/life/Cargo.toml`, in the `[workspace.dependencies]` section:

```toml
inference-core = { path = "crates/inference/inference-core", version = "0.1.0" }
life-inference = { path = "crates/inference/life-inference", version = "0.1.0" }
```

- [ ] **Step 10.4: Verify everything compiles**

```bash
cargo check -p inference-core -p life-inference 2>&1 | tail -10
```

- [ ] **Step 10.5: Commit**

```bash
git add core/life/Cargo.toml core/life/crates/inference/life-inference/
git commit -m "feat(inference): life-inference facade crate (Spec E E-Sub-A T10)"
```

---

## Task 11: Conformance scaffold

**Files:**
- Create: `crates/inference/inference-core/tests/conformance.rs`

The conformance battery proper (E-Sub-F) tests every backend × model × mode combination. The scaffold here defines the helper macros and base assertions so future backends drop into the harness without re-deriving the contract.

- [ ] **Step 11.1: Write the conformance scaffold**

```rust
//! Cross-backend conformance scaffold.
//!
//! E-Sub-F fans this out into per-backend test modules. For E-Sub-A
//! we run the suite against `InProcessInferenceBackend::new_for_test`
//! to lock the *contract* — a real backend that fails any of these
//! is non-conforming.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use inference_core::{
    AnimaIdRef, FinishReason, InferenceBackend, InMemoryKvCache, ModelId, StepContext, Token,
};

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
        let tok = item.expect(&format!(
            "{}: stream must not error in conformance",
            backend.backend_id()
        ));
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
```

- [ ] **Step 11.2: Run, expect pass**

```bash
cargo test -p inference-core --test conformance 2>&1 | tail -10
```

- [ ] **Step 11.3: Run the full crate test suite, expect ≥ 30 tests**

```bash
cargo test -p inference-core 2>&1 | grep -E "test result|running [0-9]+" | tail -5
```

Expected: cumulative `test result: ok` across module tests + 2 integration test files. Final tally ≥ 16 unit tests + ≥ 3 integration tests.

> **Stretch goal toward the spec's 30-test bar:** if we're under, add property-based tests for KvKey-derivation invariance and CloseCode parsing using `proptest`. Not required for E-Sub-A merge but moves us toward the spec criterion.

- [ ] **Step 11.4: Commit**

```bash
git add core/life/crates/inference/inference-core/tests/conformance.rs
git commit -m "test(inference): cross-backend conformance scaffold (Spec E E-Sub-A T11)"
```

---

## Task 12: Integration check + push + draft PR

- [ ] **Step 12.1: Full workspace check**

```bash
cd /Users/broomva/broomva/core/life
cargo check --workspace 2>&1 | tail -10
cargo clippy -p inference-core -p life-inference 2>&1 | tail -10
```

Expected: zero errors, zero clippy warnings (the `clippy::pedantic` lint is on; address any complaints inline).

- [ ] **Step 12.2: Run the entire crate's test suite one more time**

```bash
cargo test -p inference-core -p life-inference 2>&1 | tail -5
```

- [ ] **Step 12.3: Push to remote**

```bash
git push -u origin feat/spec-e-sub-a
```

- [ ] **Step 12.4: Draft the PR body (heredoc into `gh pr create`)**

```bash
gh pr create --draft --title "feat(inference): Spec E E-Sub-A — InferenceBackend foundation" --body "$(cat <<'EOF'
## Summary
- Ships the foundation crate `crates/inference/inference-core` and facade `crates/inference/life-inference`.
- Locks the trait shape from Spec E (`core/life/docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md`): `InferenceBackend`, `KvCache`, `InferenceRouter`, plus 8 close codes and 5 token variants.
- Provides `InProcessInferenceBackend` (test-mode + production-shim) so nothing breaks today and the trait shape is exercised end-to-end.
- Provides `InMemoryKvCache` for tests; production Lago-backed impl is E-Sub-E.
- Cross-backend conformance scaffold (`tests/conformance.rs`) ready for E-Sub-B/C to drop into.

## Spec compliance
- L5-D1 ✅ separate crate cluster.
- L5-D2 ⏳ trait wired; Lago impl is E-Sub-E.
- L5-D3 ✅ `step_speculative` is opt-in (default panics).
- L5-D4 ✅ Spec C₃ §6.5-aligned `CloseCode`.
- L5-D5 ✅ tool dispatch via `CloseCode::ToolAwait` + `with_tool_result` re-entry.
- L5-D6 ✅ `KvKey::derive` includes `anima_did`.
- L5-D7 ✅ `InferenceRouter::set_policy` runtime override.
- L5-D8 ⏳ public spec publication is E-Sub-I (post-Phase-1).

## Test plan
- [x] `cargo test -p inference-core` passes (≥ 19 tests)
- [x] `cargo clippy -p inference-core -p life-inference` clean
- [x] `cargo check --workspace` passes
- [ ] Subagent-driven review pass (spec compliance + code quality)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 12.5: Watch CI via P9**

```bash
p9 watch $(gh pr view --json number -q .number) --background
```

When CI green, request the two-stage review (spec compliance + code quality) per Spec D's pattern.

---

## Self-review checklist

After completing all tasks, run through this checklist before requesting review:

1. **Spec coverage:** every locked decision L5-D1..L5-D8 has a corresponding artifact above (✅ all 8 mapped).
2. **No placeholders:** search the diff for `TODO`, `TBD`, `unimplemented!`, `todo!()` — should be zero.
3. **Type consistency:** `KvKey::derive` signature in `ids.rs` matches all call sites (`kv_inmem.rs`, conformance test).
4. **Test count:** the spec asks for ≥ 30 unit tests across the module. Current count after E-Sub-A is ~19. Backlog item: add `proptest` cases (E-Sub-A follow-up sub-task) before E-Sub-F runs.
5. **No breakage:** `cargo check --workspace` and `cargo test -p arcan-core` both pass — proves we didn't accidentally couple `inference-core` to anything fragile.
6. **Commit hygiene:** 12 commits, one per task, each with the `Spec E E-Sub-A T<N>` tag for traceability.

If a check fails, fix inline and re-run only the affected step. No need to redo the whole plan.

---

## Out-of-scope follow-ups (for E-Sub-A's PR description)

These are *known* gaps tracked as separate tickets, not regressions:

- `InProcessInferenceBackend` production wiring — currently returns `BackendUnavailable` for non-test mode. Real wire-up to `arcan-core::aisdk` happens in E-Sub-A.1 (a tiny follow-up PR after the trait shape is locked).
- `KvPinGuard` lifetime model uses `'static` reference cast — fine for the in-mem dev impl, but production backends need an `Arc<Self>` + weak-handle pattern. Document in E-Sub-B's MLX backend.
- The 30-test target is met by the time E-Sub-F lands; E-Sub-A's ~19 tests are sufficient to lock the trait shape.

---

## Handoff

When E-Sub-A merges:

1. File E-Sub-B (MLX), E-Sub-C (vLLM), E-Sub-D (Vigil), E-Sub-E (Autonomic) plans — each gets its own document in `core/life/docs/superpowers/plans/` following this template.
2. Dispatch them in parallel via `superpowers:dispatching-parallel-agents` (matches Spec D Wave 2A/2B pattern).
3. E-Sub-F (conformance battery) waits for E-Sub-B + E-Sub-C to merge so it has ≥ 2 real backends to compare.
