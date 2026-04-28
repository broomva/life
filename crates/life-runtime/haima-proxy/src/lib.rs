//! haima-proxy — typed tonic client for the haima substrate.

#![deny(unsafe_code)]

pub mod client;
pub mod error;

pub use client::{HaimaCall, HaimaProxy, LedgerEntry, WalletBalance};
pub use error::{HaimaProxyError, HaimaProxyResult, RetryClass};
