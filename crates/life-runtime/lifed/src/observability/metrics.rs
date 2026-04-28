//! Metric registration helpers per Spec C₂ §9.3.
//!
//! Registers the canonical lifed metric series against the global
//! OpenTelemetry meter (vigil's exporter wires the rest). Series names
//! follow the `life.daemon.*` and `life.{session,saga}.*` prefixes
//! defined in §9.3.
//!
//! Sub-phase D ships the registry shape; the values are bumped from
//! the dispatch path (pool acquire/release, breaker state transitions,
//! fanout broadcasts, saga lifecycle).

use opentelemetry::global;
use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};

/// All lifed metric series. Held inside `VigilGuard` for the daemon's
/// lifetime so the meter provider stays alive.
pub struct LifedMetrics {
    pub dispatch_count: Counter<u64>,
    pub dispatch_duration_ms: Histogram<u64>,
    pub handler_duration_ms: Histogram<u64>,
    pub session_active: Gauge<i64>,
    pub session_created: Counter<u64>,
    pub session_destroyed: Counter<u64>,
    pub session_replay_seconds: Histogram<f64>,
    pub cache_size: Gauge<i64>,
    pub cache_evictions: Counter<u64>,
    pub breaker_state: Gauge<i64>,
    pub semaphore_inflight: Gauge<i64>,
    pub slow_stream_total: Counter<u64>,
    pub saga_inflight: Gauge<i64>,
    pub saga_completed: Counter<u64>,
    pub saga_compensation_failed: Counter<u64>,
}

/// Build the metric registry against the global meter. Call this once
/// during `observability::init`. Per OpenTelemetry semantics, instruments
/// are cheap to construct repeatedly — but holding them on `VigilGuard`
/// keeps allocations to once per daemon.
pub fn register() -> LifedMetrics {
    let meter: Meter = global::meter("lifed");
    LifedMetrics {
        dispatch_count: meter.u64_counter("life.daemon.dispatch.count").build(),
        dispatch_duration_ms: meter
            .u64_histogram("life.daemon.dispatch.duration_ms")
            .build(),
        handler_duration_ms: meter
            .u64_histogram("life.daemon.handler.duration_ms")
            .build(),
        session_active: meter.i64_gauge("life.session.active").build(),
        session_created: meter.u64_counter("life.session.created_total").build(),
        session_destroyed: meter.u64_counter("life.session.destroyed_total").build(),
        session_replay_seconds: meter.f64_histogram("life.session.replay_seconds").build(),
        cache_size: meter.i64_gauge("life.daemon.cache.size").build(),
        cache_evictions: meter
            .u64_counter("life.daemon.cache.evictions_total")
            .build(),
        breaker_state: meter.i64_gauge("life.daemon.breaker_state").build(),
        semaphore_inflight: meter.i64_gauge("life.daemon.semaphore.inflight").build(),
        slow_stream_total: meter.u64_counter("life.daemon.slow_stream_total").build(),
        saga_inflight: meter.i64_gauge("life.saga.inflight").build(),
        saga_completed: meter.u64_counter("life.saga.completed_total").build(),
        saga_compensation_failed: meter
            .u64_counter("life.saga.compensation_failed_total")
            .build(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_returns_all_series() {
        // The mere fact that register() returns indicates every
        // instrument was successfully built — opentelemetry returns
        // a no-op meter when no provider is set, but the build()
        // calls still succeed.
        let m = register();
        // Smoke: increment a counter to make sure the type matches.
        m.dispatch_count.add(1, &[]);
        m.session_created.add(1, &[]);
        m.breaker_state.record(0, &[]);
    }
}
