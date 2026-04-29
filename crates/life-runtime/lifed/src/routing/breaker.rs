//! Re-export shim. The breaker primitives now live in
//! `life-runtime-pool` (Sub-phase E push-down) so the four `*-proxy`
//! crates can own pools without depending on lifed.
//!
//! Sub-phase E history: this module previously held the hand-rolled
//! breaker; the body moved to `life-runtime-pool::breaker`. The
//! `lifed::routing::breaker` path remains as a stable namespace for
//! existing consumers (integration tests, admin handlers, observability
//! call sites).

pub use life_runtime_pool::breaker::{
    BreakerState, CircuitBreaker, FAILURE_THRESHOLD, OPEN_DURATION, RATE_MIN_SAMPLES,
    RATE_THRESHOLD, RATE_WINDOW,
};
