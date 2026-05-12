//! `soma` library surface — re-exports for integration tests and
//! downstream crates that need access to daemon internals.
//!
//! The binary entrypoint (`main.rs`) uses `use soma::*` to pull in
//! the same types. All public items here are semver-stable across
//! Phase 2; internal helpers remain `pub(crate)`.

#![deny(unsafe_code)]

pub mod admin;
pub mod bootstrap;
pub mod cli;
pub mod config;
pub mod error;
pub mod identity;
pub mod listener;
pub mod observability;
pub mod server;
pub mod shutdown;

// ── Convenience re-exports ────────────────────────────────────────────────────

pub use config::{AdminPlaneConfig, ServerConfig, SomaConfig};
pub use error::{SomaError, SomaResult};
