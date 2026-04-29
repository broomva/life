//! Re-export shim. The pool primitives now live in `life-runtime-pool`
//! (Sub-phase E push-down) so the four `*-proxy` crates can own pools
//! without depending on lifed.
//!
//! Sub-phase E history: this module previously held the [`Pool`],
//! [`PoolGuard`], [`SubstrateKind`], and [`SubstratePools`] structs; the
//! bodies moved to `life-runtime-pool::pool`. The `lifed::routing::pools`
//! path remains as a stable namespace for existing consumers (integration
//! tests, admin handlers, observability call sites).

pub use life_runtime_pool::pool::{
    Pool, PoolGuard, SubstrateKind, SubstratePools, SubstratePoolsInitial,
};
