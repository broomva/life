//! Typed Rust client for the Life Kernel wire surface.
//!
//! `LifeClient` connects over Unix socket (default), vsock (feature
//! `vsock`), or TCP (feature `tcp`, dev only) to a tonic server hosting
//! the `life.kernel.v1` services. Per-service handles match the
//! `aios-protocol` port trait signatures 1:1, wrapping the generated
//! tonic client stubs with ergonomic typed errors.
//!
//! See `docs/superpowers/specs/2026-04-24-life-kernel-facade-design.md` §7.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod connect;
pub mod error;
pub mod services;

pub use connect::{LifeClient, LifeTransport};
pub use error::{LifeClientError, LifeResult};
