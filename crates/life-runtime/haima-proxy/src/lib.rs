//! haima-proxy — typed tonic client for the haima substrate.

#![deny(unsafe_code)]

pub mod client;
pub mod error;

pub use client::{
    HaimaCall, HaimaProxy, LedgerEntry, LedgerGuardedStream, Pooled, WalletBalance, X402PayOutcome,
};
pub use error::{HaimaProxyError, HaimaProxyResult, RetryClass};
