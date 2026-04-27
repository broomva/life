//! gRPC service implementations.
//!
//! Each module hosts the tonic service trait impl for one public-plane
//! namespace (`agent`, `events`, `wallet`, `identity`) or admin-plane
//! namespace (under `admin/`).
//!
//! Per Spec C₂ §1, every public RPC handler reads the capability token,
//! performs a single substrate route or saga dispatch, and returns. ≤20 LOC
//! handler rule applies — anything resembling business logic happens in
//! the substrate, never here.

pub mod agent;
pub mod events;
pub mod wallet;
// pub mod identity;     // sub-phase B (B14)
// pub mod admin;        // sub-phase C (C2–C5)
