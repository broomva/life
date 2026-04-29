//! lifegw — Life Runtime edge gateway daemon.
//!
//! The unprivileged stateless internet-facing daemon that terminates TLS,
//! verifies Tier-1 identity JWTs, mints Tier-2 capability tokens, and forwards
//! `life.v1.*` unary RPCs to lifed via UDS.
//!
//! See `docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md` for the
//! detailed design. This crate is the binary's private library — it exists to
//! share types between `main.rs` and the integration tests under `tests/`. No
//! public API is exposed beyond what tests need.
//!
//! # Sub-phase A scope
//!
//! Per Spec C₃ §12.A:
//!
//! - TLS bind via rustls.
//! - Dev-mode JWT acceptance (`Bearer dev-token-for-{user_id}`).
//! - Tier-2 capability token mint via a static in-process P-256 keystore.
//! - `tonic-web` Connect protocol layer fronting a transparent proxy to
//!   `/run/life/life.sock` (lifed).
//! - `/healthz` endpoint that probes upstream lifed reachability.
//!
//! Real ES256 + Vercel JWKS land in Sub-phase B; WS in C; rate-limit in D;
//! production hardening in E.

#![deny(unsafe_code)]

pub mod admin;
pub mod auth;
pub mod bootstrap;
pub mod cli;
pub mod config;
pub mod error;
pub mod listener;
pub mod observability;
pub mod proxy;
pub mod services;
pub mod shutdown;

pub use error::{LifegwError, LifegwResult};
