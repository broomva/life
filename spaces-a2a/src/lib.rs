//! spaces-a2a — A2A protocol bridge for Life Spaces.
//!
//! Exposes Life agents as A2A-compatible services via:
//! - HTTP JSON-RPC 2.0 endpoints
//! - Well-known Agent Card discovery (RFC 8615)
//! - gRPC transport (tonic)
//!
//! Architecture:
//!
//! ```text
//! External A2A Agent
//!     ↓ JSON-RPC / gRPC
//! spaces-a2a (this crate)
//!     ↓ SpacetimeDB SDK (via bridge module)
//! SpacetimeDB Module (spaces)
//!     ↓ Pub/Sub
//! Life Agents (Arcan, Lago, etc.)
//! ```

pub mod agent_card;
pub mod bridge;
pub mod grpc;
pub mod jsonrpc;
pub mod types;
