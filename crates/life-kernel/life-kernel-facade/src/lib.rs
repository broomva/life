//! Life Kernel Facade — Spec B.1 Phase 1.
//!
//! This crate hosts two concerns:
//!
//! 1. **HTTP proxy implementations** of `aios_protocol::ports` traits for
//!    every downstream Life daemon — `EventsProxy` (lagod), `SessionProxy`
//!    and `ApprovalsProxy` (arcand). Each proxy is a thin `reqwest`
//!    client that projects the daemon's existing HTTP/SSE surface onto
//!    the canonical port trait. The proxies do **not** link any daemon's
//!    runtime — they consume only the port trait from `aios-protocol`
//!    and the DTO schemas from `*-api-schema` crates.
//!
//! 2. **Generic tonic-service-trait adapters** that translate
//!    `life-kernel-proto` wire requests into port-trait calls. Each
//!    adapter is parameterised over its port trait so `soma` (Spec A
//!    Phase 2) can register any combination of HTTP proxy impls (for
//!    capabilities hosted by external daemons) or in-process impls
//!    (e.g. `PolicyService<StaticPolicyGate>` from `life-kernel-gate`).
//!
//! # Boundaries
//!
//! - `life-kernel-facade` MUST NOT depend on any daemon's runtime
//!   crate. See `scripts/verify_dependencies_lifed.sh`.
//! - `life-kernel-facade` MUST NOT write to Lago; the facade is pure
//!   proxy + translation. Downstream daemons own their own event
//!   emission. The facade's own observability is Vigil-only
//!   (`life.facade.*` spans).

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod arcand;
pub mod config;
pub mod convert;
pub mod error;
pub mod lagod;
pub mod retry;
pub mod services;
pub mod telemetry;

// Public re-exports (DaemonEndpoints, FacadeError, FacadeResult) are wired in
// Task 10 once those types exist.
