//! # ergon-life-hooks — Life-native auto-registered hooks
//!
//! This crate ships the four hooks the spec calls "auto-registered":
//!
//! | Hook | Event | Adapter trait | Implemented in |
//! |---|---|---|---|
//! | [`PraxisCapabilityHook`] | `on_pre_tool_use` | [`CapabilityResolver`] | arcan adapter (BRO-1001) against `aios_protocol::PolicySet` |
//! | [`AutonomicBudgetHook`] | `on_pre_inference` | [`BudgetGate`]           | arcan adapter against `autonomic::AutonomicGatingProfile` |
//! | [`NousScoreHook`]        | `on_post_inference`| [`ResponseScorer`]       | arcan adapter against `nous_core::NousEvaluator` |
//! | [`AnimaAttestHook`]      | `on_workflow_start` / `on_workflow_end` | [`SoulAttester`] | arcan adapter against `anima_core::AgentSoul` (or its event-store sibling) |
//!
//! ## Design — the adapter-trait pattern
//!
//! Each hook takes an `Arc<dyn <Adapter>Trait>` injected at construction
//! time. The adapter trait is **owned by this crate**, not by the substrate
//! crate. This gives us:
//!
//! 1. **Zero substrate deps in this crate.** ergon-life-hooks compiles
//!    against `ergon` only. It pulls in no `anima-core`, no
//!    `autonomic-core`, no `nous-core`, no `aios-protocol-extension`
//!    machinery. The substrate dep lives in BRO-1001 (the arcan adapter)
//!    where it belongs.
//!
//! 2. **Full mockability.** Each hook is unit-testable with a tiny
//!    `Adapter` mock. No substrate ceremony.
//!
//! 3. **Substrate API swaps don't cascade.** When `anima-core` changes its
//!    signing API, only the arcan adapter's `SoulAttester` impl moves —
//!    not the hook, not its tests, not ergon, not the workflow author.
//!
//! ## Why a separate crate (not in ergon)?
//!
//! `ergon` is the vendor-neutral harness primitive. These four hooks
//! encode **Life's** specific governance choices: capability gating,
//! budget enforcement, metacognitive scoring, soul attestation. A future
//! consumer of ergon (e.g., a TS port, a different agent OS) would
//! plausibly want a different set of auto-hooks. Keeping them in their
//! own crate makes that clean.
//!
//! ## Status
//!
//! Linear: [BRO-1000](https://linear.app/broomva/issue/BRO-1000).
//! Spec: `core/life/docs/superpowers/specs/2026-05-05-ergon-v0.1.md` §3.8.
//!
//! ## What this crate does NOT do
//!
//! - **Construct the auto-hook registry**: that's the arcan adapter's job
//!   (BRO-1001), since registry construction needs substrate handles.
//! - **Implement the adapter traits**: again, BRO-1001.
//! - **Wire ordering**: the spec's "auto-hooks fire before user hooks"
//!   ordering is enforced by the arcan adapter when it appends user
//!   hooks to the registry it built from the auto-hooks here.

#![doc(html_no_source)]

pub mod attestation;
pub mod budget;
pub mod capability;
pub mod score;

pub use attestation::{AnimaAttestHook, SoulAttester};
pub use budget::{AutonomicBudgetHook, BudgetGate};
pub use capability::{CapabilityResolver, PraxisCapabilityHook};
pub use score::{NousScoreHook, ResponseScorer};
