//! Facade re-export for the Life inference layer (Spec E).
//!
//! Mirrors `life-anima` and `life-aios` — downstream apps depend on
//! `life-inference` rather than picking sub-crates by hand. Backend
//! enable/disable goes through this crate's feature flags.

#![forbid(unsafe_code)]

pub use inference_core::*;
