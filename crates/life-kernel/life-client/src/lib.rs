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

impl LifeClient {
    /// Handle over the `life.Kernel` service.
    pub fn kernel(&self) -> services::Kernel<'_> {
        services::Kernel::new(self)
    }

    /// Handle over the `life.Events` service.
    pub fn events(&self) -> services::Events<'_> {
        services::Events::new(self)
    }

    /// Handle over the `life.Session` service.
    pub fn session(&self) -> services::Session<'_> {
        services::Session::new(self)
    }

    /// Handle over the `life.Approvals` service.
    pub fn approvals(&self) -> services::Approvals<'_> {
        services::Approvals::new(self)
    }

    /// Handle over the `life.Policy` service.
    pub fn policy(&self) -> services::Policy<'_> {
        services::Policy::new(self)
    }
}
