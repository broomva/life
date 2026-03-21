//! # Anima — The Self Layer for the Life Agent OS
//!
//! Anima (Latin: soul, inner self) provides the foundational identity
//! primitives for the Life Agent Operating System. While every other
//! crate answers *what the agent does*, Anima answers **who the agent is**.
//!
//! ## Core Types
//!
//! - [`AgentSoul`] — The immutable origin: lineage, values, cryptographic root.
//!   Created once, never modified. Persisted as a genesis event in Lago.
//!
//! - [`AgentIdentity`] — The cryptographic proof: Ed25519 (auth) + secp256k1
//!   (economics) dual keypair. Resolves to Agent Auth Protocol for web/API
//!   and to Haima for on-chain payments.
//!
//! - [`AgentBelief`] — The mutable self-model: capabilities, constraints,
//!   trust scores, reputation, economic state. Projected from events.
//!   Constrained by the soul's [`PolicyManifest`].
//!
//! - [`AgentSelf`] — The composite: Soul + Identity + Belief. The single
//!   type consumed by all Life crates.
//!
//! ## Biological Metaphor
//!
//! | Crate | Metaphor | Role |
//! |-------|----------|------|
//! | aiOS | DNA | Kernel contract |
//! | **Anima** | **Soul** | **Identity, belief, values** |
//! | Arcan | Mind | Runtime cognition |
//! | Lago | Memory | Persistence |
//! | Autonomic | Nervous system | Self-regulation |
//! | Haima | Blood | Economic circulation |
//! | Praxis | Hands | Tool execution |
//! | Vigil | Senses | Observability |
//! | Spaces | Voice | Communication |
//!
//! ## Resolution Chain
//!
//! ```text
//! AgentSelf
//! ├── soul (immutable) ──── persisted in Lago as genesis event
//! ├── identity
//! │   ├── auth (Ed25519) ── Agent Auth Protocol, JWT, MCP
//! │   └── wallet (secp256k1) ── Haima payments, on-chain DID
//! └── beliefs (mutable) ── capabilities, trust, economic state
//! ```

pub mod agent_self;
pub mod belief;
pub mod error;
pub mod event;
pub mod identity;
pub mod policy;
pub mod soul;

// Re-exports for convenience
pub use agent_self::AgentSelf;
pub use belief::AgentBelief;
pub use error::{AnimaError, AnimaResult};
pub use event::AnimaEventKind;
pub use identity::AgentIdentity;
pub use policy::PolicyManifest;
pub use soul::{AgentSoul, SoulBuilder};
