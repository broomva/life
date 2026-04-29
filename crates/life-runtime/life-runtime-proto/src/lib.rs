//! Generated proto types for `life.v1.*` (public plane) and
//! `life.admin.v1.*` (admin plane).
//!
//! Mirrors the `aios-proto` pattern from M3 (`docs/superpowers/plans/2026-04-25-m3-proto-consolidation.md`):
//! a thin crate that hosts `tonic::include_proto!` modules. The
//! `core/life/proto/life/v1/*.proto` and `core/life/proto/life/admin/v1/*.proto`
//! sources are the canonical wire surface; this crate exists to give
//! lifed (and any future SDK builder) typed Rust bindings.

#![deny(unsafe_code)]
#![allow(clippy::all)]
#![allow(missing_docs)]

pub mod life {
    pub mod v1 {
        tonic::include_proto!("life.v1");
    }
    pub mod admin {
        pub mod v1 {
            tonic::include_proto!("life.admin.v1");
        }
        // Sub-phase D (D2): lifegw admin plane.
        pub mod gw {
            pub mod v1 {
                tonic::include_proto!("life.admin.gw.v1");
            }
        }
    }
}

// Re-export aios-proto for callers that want a single import path.
pub use aios_proto::aios as aios_v1;
