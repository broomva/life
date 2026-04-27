//! life.admin.v1.* — admin-plane services.
//!
//! - `runtime`        — `Runtime` (HealthCheck, Sessions*, IdempotencyLookup).
//! - `saga`           — `Saga` (ListInflight, Show, ForceCompensate-stub).
//! - `routing_cache`  — `RoutingCache` (Dump, Evict, RebuildFromLago-stub).
//! - `policy`         — closed-by-default `AdminPolicy` table.

pub mod policy;
pub mod routing_cache;
pub mod runtime;
pub mod saga;

pub use policy::{AdminOp, AdminPolicy};
pub use routing_cache::RoutingCacheAdminService;
pub use runtime::RuntimeAdminService;
pub use saga::SagaAdminService;
