//! lago-proxy — typed tonic client for the lago substrate.

#![deny(unsafe_code)]

pub mod client;
pub mod error;

pub use client::{EventGuardedStream, LagoCall, LagoProxy, Pooled};
pub use error::{LagoProxyError, LagoProxyResult, RetryClass};
