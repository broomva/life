//! Arcan Middleware implementation for Nous.
//!
//! `NousMiddleware` implements `arcan_core::runtime::Middleware` and
//! runs registered evaluators at each hook point.

pub mod middleware;

pub use middleware::NousMiddleware;
