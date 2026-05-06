//! # ergon — Life's agent-harness primitive
//!
//! Ergon is Life's Layer-2 agent-harness: the trait set that lets a Broomva
//! developer write a [`Workflow`] in Rust whose deterministic outer body
//! orchestrates autonomous inner LLM steps, integrated end-to-end with the
//! Life substrate (praxis tools, lago events, anima identity, autonomic
//! budgets, nous scoring, vigil traces, lifegw delivery).
//!
//! ## Naming
//!
//! `ergon` (Greek *ἔργον*, work / function / characteristic activity). Pairs
//! philosophically with `praxis` (Greek *πρᾶξις*, doing) and `nous` (Greek
//! *νοῦς*, mind). Three-way semantic fit: nous selects → praxis does → ergon
//! is the work performed.
//!
//! ## Layered position
//!
//! ```text
//! Layer 4 — Life (Agent OS substrate)
//! Layer 3 — arcan (runtime daemon, OperatingMode FSM, capability gating)
//! Layer 2 — ergon (HARNESS — owned by this crate)
//! Layer 1 — arcan-provider (model wire connectors, multi-vendor)
//! Layer 0 — model (Anthropic / OpenAI / Bedrock / etc.)
//! ```
//!
//! ## Status — v0.1 (in progress)
//!
//! This crate is shipping incrementally per
//! `docs/superpowers/specs/2026-05-05-ergon-v0.1.md` (spec §12 work order):
//!
//! - **Landed in this slice**: [`error`], [`role`], [`stream`] — pure data
//!   shapes with no Life-substrate dependencies (the foundation layer).
//! - **Coming next**: [`hook`] (trait + registry), [`step`] (`Step` +
//!   `StepCtx` + `RuntimeHandle`), [`workflow`] (`Workflow` +
//!   `WorkflowExecutor`), four auto-hooks (capability / budget / score /
//!   attestation), arcan adapter, lifed route, bookkeeping-judge port.
//!
//! See [Linear project BRO-994](https://linear.app/broomva/project/ergon-agent-harness-primitive-ca2a51a0fba1)
//! for the implementation tracker.

#![doc(html_no_source)]

pub mod error;
pub mod role;
pub mod stream;

pub use error::{ErgonError, Result};
pub use role::{Role, RoleScope};
pub use stream::{BufferSink, FanoutSink, StopReason, StreamEvent, StreamSink};

/// Re-export of [`aios_protocol::ids::SessionId`] — the canonical session
/// identifier used throughout the Life substrate. Ergon does not define its
/// own session type; it borrows the kernel-contract one.
pub use aios_protocol::ids::SessionId;
