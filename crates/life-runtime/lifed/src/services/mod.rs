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
//!
//! Admin-plane handlers (under `admin/`) may exceed the 20-LOC budget
//! since dump-and-filter ops naturally take more lines, but never hold a
//! lock across `await`.

pub mod admin;
pub mod agent;
pub mod events;
pub mod identity;
pub mod wallet;
