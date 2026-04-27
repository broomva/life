//! lago-proxy — typed tonic client for the lago substrate.

#![deny(unsafe_code)]

pub mod client;
pub mod error;

pub use client::{LagoCall, LagoProxy};
pub use error::{LagoProxyError, LagoProxyResult};
