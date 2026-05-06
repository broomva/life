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
//! - **Landed**: [`error`], [`role`], [`stream`] (foundation layer);
//!   [`model`] (wire types — `Message`, `ContentBlock`, `ToolCall`,
//!   `ToolResult`, `ToolDefinition`, `ModelRequest`, `ModelResponse`,
//!   `Usage`); [`hook`] (8-event lifecycle trait + `HookRegistry` +
//!   `HookCtx` + outcome types); [`runtime`] (`Provider`, `ToolRegistry`,
//!   `RuntimeHandle` traits — the seam to the host runtime); [`step`]
//!   (`Step`, `StepCtx`, `InferenceRequest`, autonomous loop body);
//!   [`workflow`] (`Workflow` trait, `WorkflowExecutor`, `SkillSet`).
//! - **Coming next**: substrate sinks (`LagoSink`, `VigilSink`,
//!   `LifegwSink` — in a sibling `ergon-life-sinks` crate), the four
//!   auto-hooks (capability / budget / score / attestation — in the
//!   `ergon-life-hooks` sibling crate), the arcan adapter that
//!   translates between ergon's traits and substrate types, the lifed
//!   route, the bookkeeping-judge port.
//!
//! See [Linear project BRO-994](https://linear.app/broomva/project/ergon-agent-harness-primitive-ca2a51a0fba1)
//! for the implementation tracker.

#![doc(html_no_source)]

pub mod error;
pub mod hook;
pub mod model;
pub mod role;
pub mod runtime;
pub mod step;
pub mod stream;
pub mod workflow;

pub use error::{ErgonError, Result};
pub use hook::{Hook, HookCtx, HookOutcome, HookRegistry, InferenceHookOutcome, ToolHookOutcome};
pub use model::{
    ContentBlock, Message, MessageRole, ModelRequest, ModelResponse, ToolCall, ToolDefinition,
    ToolResult, Usage,
};
pub use role::{Role, RoleScope};
pub use runtime::{Provider, RuntimeHandle, ToolRegistry};
pub use step::{DEFAULT_INFERENCE_MAX_TURNS, InferenceRequest, Step, StepCtx};
pub use stream::{BufferSink, FanoutSink, StopReason, StreamEvent, StreamSink};
pub use workflow::{EmptySkillSet, SkillSet, Workflow, WorkflowExecutor};

/// Re-export of [`aios_protocol::ids::SessionId`] — the canonical session
/// identifier used throughout the Life substrate. Ergon does not define its
/// own session type; it borrows the kernel-contract one.
pub use aios_protocol::ids::SessionId;
