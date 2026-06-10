//! lifed — Life Runtime facade-aggregator daemon.
//!
//! See `docs/superpowers/specs/2026-04-26-spec-c2-lifed-facade.md` for the
//! design and `docs/superpowers/plans/2026-04-26-m5-lifed-build.md` for
//! the implementation plan.
//!
//! This crate is the binary's private library — it exists to share types
//! between `main.rs` and the integration tests under `tests/`. No public
//! API is exposed beyond what tests need.

#![deny(unsafe_code)]

pub mod auth;
pub mod bootstrap;
pub mod cli;
pub mod config;
pub mod dev_mocks;
pub mod error;
pub mod idempotency;
pub mod listener;
pub mod observability;
pub mod route;
pub mod routing;
pub mod saga;
pub mod services;
pub mod shutdown;

pub use error::{LifedError, LifedResult};
