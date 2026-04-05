//! # Anima Lago — Persistence Bridge
//!
//! This crate bridges Anima's types to Lago's event-sourced persistence:
//!
//! - **Genesis**: How a soul enters the journal as its first event
//! - **Projection**: How beliefs are reconstructed by folding events
//!
//! All Anima events use `EventKind::Custom` with the `"anima."` namespace,
//! following the same pattern as Haima (`"finance."`) and Autonomic (`"autonomic."`).

pub mod genesis;
pub mod projection;

pub use genesis::{create_genesis_event, reconstruct_soul};
pub use projection::{fold, replay};
