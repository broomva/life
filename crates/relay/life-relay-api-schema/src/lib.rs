//! HTTP API DTOs for life-relayd — schema-only crate.
//!
//! This crate intentionally contains **no runtime code**. It exists so
//! `life-kernel-facade` can depend on typed request/response shapes without
//! pulling in life-relayd's server runtime. Types are filled in by Phase 0 tasks
//! that mirror the canonical HTTP surface at
//! `core/life/crates/relay/life-relayd/src/`.

#![forbid(unsafe_code)]
