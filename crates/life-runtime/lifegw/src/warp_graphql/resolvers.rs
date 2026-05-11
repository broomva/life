//! Resolver tiers per Lumen spec L6-D4.
//!
//! - tier1: ~15 ops, must implement (working agent)
//! - tier2: ~15 ops, production UX
//! - tier3_stubs: ~90 ops, return safe defaults

pub mod tier1;
pub mod tier2;
pub mod tier3_stubs;
