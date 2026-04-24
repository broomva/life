//! HTTP API DTOs for nousd — schema-only crate.
//!
//! This crate intentionally contains **no runtime code**. It exists so
//! `life-kernel-facade` can depend on typed request/response shapes without
//! pulling in nousd's server runtime. Types mirror the canonical HTTP surface
//! at `core/life/crates/nous/nousd/src/` and are re-exported from
//! `aios-protocol::evaluation`.

#![forbid(unsafe_code)]

pub use aios_protocol::evaluation::*;
