//! Routing cache + multi-tab fan-out registry + per-substrate pools.
//!
//! Spec C₂ §6 (cache + fanout) and §7 (pools + breakers). Sub-phase A
//! shipped the cache without eviction; sub-phase B added eviction (B8);
//! sub-phase D adds cold-start replay from lago (D2), per-substrate
//! pools (D1), and the hand-rolled circuit breaker (D1).

pub mod breaker;
pub mod cache;
pub mod fanout;
pub mod pools;

pub use cache::{RouteEntry, RoutingCache, SessionStatus};
