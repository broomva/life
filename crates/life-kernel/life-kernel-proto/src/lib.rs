//! Wire contract for the soma kernel daemon.
//!
//! The [`pb`] module holds the prost-generated message types and the
//! tonic-generated `KernelService` server/client stubs. A private
//! `convert` module bridges those generated types to the canonical
//! `aios_protocol` types consumed elsewhere in the workspace; its
//! public error type surfaces as [`ConvertError`].
//!
//! ## M3 — canonical proto tree
//!
//! Generated from `core/life/proto/life/kernel/v1/kernel.proto` (canonical
//! tree per master spec C M3) plus the legacy v0 service protos at
//! `proto/*.proto` (migrated in M3.5 / M4). The kernel proto imports
//! `aios.v1.*` types from the `aios-proto` crate via the
//! `extern_path` directive in `build.rs`, so identifier message types
//! (`VmId`, `VmSnapshotId`, `BackendId`, `SessionId`, `AgentId`) are
//! re-exported from `aios_proto::aios::v1` rather than redefined locally.
//!
//! ## Transport choice (tonic over ttrpc)
//!
//! See `build.rs` for the full rationale — in short, `ttrpc-codegen`
//! is incompatible with `prost = "0.14"`, and the workspace already
//! standardises on tonic. Tonic over a Unix domain socket will match
//! the deployment shape the soma daemon needs in Phase 2.

#![deny(unsafe_code)]

/// Generated prost + tonic code for the `life.kernel.v1` package.
///
/// The generated code is wrapped in broad `#[allow(...)]` attributes
/// because it intentionally ignores our workspace-level style lints
/// (clippy pedantic groups, missing-docs, etc.). Treat everything in
/// this module as an opaque wire contract — interact with it through
/// the private `convert` bridges (surfaced as `impl TryFrom<…>` on the
/// canonical `aios_protocol` types) rather than reaching in directly.
///
/// M3 (BRO-928): the package was renamed `broomva.life.kernel.v1` →
/// `life.kernel.v1`. Wire field tags + service shape are unchanged; only
/// the Rust module path moved. A deprecated `broomva_life_kernel_v1`
/// alias is preserved below for one minor version.
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod pb {
    tonic::include_proto!("life.kernel.v1");
}

/// Deprecated alias for the pre-M3 package path. Remove in 0.4.
///
/// New code should use [`pb`] directly. Existing internal callers see no
/// behavioural change — message types remain wire-compatible across the
/// rename.
#[deprecated(
    since = "0.3.1",
    note = "package renamed `broomva.life.kernel.v1` → `life.kernel.v1`; use `pb` instead"
)]
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod broomva_life_kernel_v1 {
    pub use super::pb::*;
}

/// Re-export the canonical `aios.v1.*` types so callers can write
/// `use life_kernel_proto::aios_v1::VmId` without depending on
/// `aios-proto` directly.
pub use aios_proto::aios::v1 as aios_v1;

/// Generated prost + tonic code for the
/// `life.admin.kernel.v1.CustodyOracle` service.
///
/// Spec D D-Sub-E: soma's admin-plane custody-oracle. Sibling of
/// [`pb::kernel_service_server::KernelService`] — mounted on the same
/// admin UDS, but in a separate proto package so per-RPC RBAC and
/// versioning evolve independently.
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod custody {
    tonic::include_proto!("life.admin.kernel.v1");
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

/// `life.Session` — SessionPort projection (served by arcand).
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod session {
    tonic::include_proto!("broomva.life.kernel.v1.session");
}

/// `life.Approvals` — ApprovalPort projection (served by arcand).
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod approvals {
    tonic::include_proto!("broomva.life.kernel.v1.approvals");
}

/// `life.Policy` — PolicyGatePort projection (served direct from soma
/// via `life-kernel-gate`).
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod policy {
    tonic::include_proto!("broomva.life.kernel.v1.policy");
}

/// `life.Tools` — v0.2 reserved stub; methods return
/// `Status::unimplemented` until the port trait is wire-projected.
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod tools {
    tonic::include_proto!("broomva.life.kernel.v1.tools");
}

/// `life.Model` — v0.2 reserved stub.
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod model {
    tonic::include_proto!("broomva.life.kernel.v1.model");
}

/// `life.Relay` — v0.2 reserved stub.
#[allow(unused_qualifications, clippy::all, missing_docs)]
pub mod relay {
    tonic::include_proto!("broomva.life.kernel.v1.relay");
}

mod convert;

pub use convert::ConvertError;
