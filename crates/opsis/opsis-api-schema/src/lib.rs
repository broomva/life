//! HTTP API DTOs for opsisd — schema-only crate.
//!
//! This crate intentionally contains **no runtime code**. It exists so
//! `life-kernel-facade` can depend on typed request/response shapes without
//! pulling in opsisd's server runtime. Types are filled in by Phase 0 tasks
//! that mirror the canonical HTTP surface at
//! `core/life/crates/opsis/opsisd/src/`.

#![forbid(unsafe_code)]
