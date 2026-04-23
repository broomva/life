//! Wire contract for the lifed kernel daemon.
//!
//! The [`pb`] module holds the prost-generated message types and the
//! tonic-generated `KernelService` server/client stubs. The [`convert`]
//! module bridges those generated types to the canonical
//! `aios_protocol` types consumed elsewhere in the workspace.
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
/// the bridges in [`crate::convert`] rather than reaching in directly.
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod pb {
    tonic::include_proto!("broomva.life.kernel.v1");
}

mod convert;

pub use convert::ConvertError;
