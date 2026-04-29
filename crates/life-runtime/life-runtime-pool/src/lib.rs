//! `life-runtime-pool` — per-substrate pool + circuit-breaker primitives
//! shared between `lifed` and the four `*-proxy` crates per Spec C₂ §7.
//!
//! Sub-phase E pushes pool ownership down into each `*Proxy` so handlers
//! drop their `pools` field; the proxy method bodies bracket internally
//! via [`Pool::acquire`] returning a [`PoolGuard`]. The breaker, the
//! semaphore-bounded permit, and the `is_half_open_trial` tag travel on
//! the guard so observability hooks know whether the dispatch was a
//! HalfOpen single-trial CAS winner.

#![deny(unsafe_code)]

pub mod breaker;
pub mod pool;

pub use breaker::{
    BreakerState, CircuitBreaker, FAILURE_THRESHOLD, OPEN_DURATION, RATE_MIN_SAMPLES,
    RATE_THRESHOLD, RATE_WINDOW,
};
pub use pool::{Pool, PoolGuard, SubstrateKind, SubstratePools, SubstratePoolsInitial};
