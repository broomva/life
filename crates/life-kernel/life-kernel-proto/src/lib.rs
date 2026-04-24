//! Wire contract for the lifed kernel daemon.
//!
//! The [`pb`] module holds the prost-generated message types and the
//! tonic-generated `KernelService` server/client stubs. A private
//! `convert` module bridges those generated types to the canonical
//! `aios_protocol` types consumed elsewhere in the workspace; its
//! public error type surfaces as [`ConvertError`].
//!
//! ## Transport choice (tonic over ttrpc)
//!
//! See `build.rs` for the full rationale — in short, `ttrpc-codegen`
//! is incompatible with `prost = "0.14"`, and the workspace already
//! standardises on tonic. Tonic over a Unix domain socket will match
//! the deployment shape the lifed daemon needs in Phase 2.

#![deny(unsafe_code)]

/// Generated prost + tonic code for the `broomva.life.kernel.v1`
/// package.
///
/// The generated code is wrapped in broad `#[allow(...)]` attributes
/// because it intentionally ignores our workspace-level style lints
/// (clippy pedantic groups, missing-docs, etc.). Treat everything in
/// this module as an opaque wire contract — interact with it through
/// the private `convert` bridges (surfaced as `impl TryFrom<…>` on the
/// canonical `aios_protocol` types) rather than reaching in directly.
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod pb {
    tonic::include_proto!("broomva.life.kernel.v1");
}

/// Shared wire DTOs — `LifeError`, `Attribution`, `Pagination`, and the
/// typed ID wrappers used across every `life.*` service proto.
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod common {
    tonic::include_proto!("broomva.life.kernel.v1.common");
}

/// `life.Events` — EventStorePort projection (served by lagod).
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod events {
    tonic::include_proto!("broomva.life.kernel.v1.events");
}

mod convert;

pub use convert::ConvertError;
