//! Routing cache + multi-tab fan-out registry.
//!
//! Spec C₂ §6. Sub-phase A ships an in-memory cache without eviction; sub-phase
//! B adds eviction (B8); sub-phase D adds cold-start replay from lago (D2).

pub mod cache;
pub mod fanout;
// pub mod pools;     // sub-phase D (D1)
// pub mod breaker;   // sub-phase D (D1)

pub use cache::{RouteEntry, RoutingCache, SessionStatus};
