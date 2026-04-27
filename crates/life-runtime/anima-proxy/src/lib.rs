//! anima-proxy — typed tonic client for the anima substrate.

#![deny(unsafe_code)]

pub mod client;
pub mod error;

pub use client::{Account, AnimaCall, AnimaProxy, Profile, SessionDescriptor};
pub use error::{AnimaProxyError, AnimaProxyResult};
