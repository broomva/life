//! arcan-proxy — typed tonic client for the arcan substrate.

#![deny(unsafe_code)]

pub mod client;
pub mod conversions;
pub mod error;

pub use client::{ArcanCall, ArcanProxy};
pub use error::{ArcanProxyError, ArcanProxyResult, RetryClass};
