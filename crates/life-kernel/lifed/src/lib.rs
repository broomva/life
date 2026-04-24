//! `lifed` library surface — re-exports for integration tests and
//! downstream crates that need access to daemon internals.
//!
//! The binary entrypoint (`main.rs`) uses `use lifed::*` to pull in
//! the same types. All public items here are semver-stable across
//! Phase 2; internal helpers remain `pub(crate)`.

#![deny(unsafe_code)]
// Phase 2 scaffold: suppress dead-code for modules exercised only by
// integration tests / BRO-900 entrypoint. Remove when BRO-900 lands.
#![allow(dead_code)]

pub mod bootstrap;
pub mod config;
pub mod error;
pub mod listener;
pub mod server;

// ── Convenience re-exports ────────────────────────────────────────────────────

pub use config::{LifedConfig, ServerConfig};
pub use error::{LifedError, LifedResult};
