//! Metric registration helpers per Spec C₂ §9.3.
//!
//! Registers the canonical lifed metric series against the global
//! OpenTelemetry meter (vigil's exporter wires the rest). Series names
//! follow the `life.daemon.*` and `life.{session,saga}.*` prefixes
//! defined in §9.3.
//!
//! Sub-phase D shipped the registry shape; Sub-phase E wires the values
//! at every dispatch point. Production code MUST go through the
//! module-level helpers ([`record_dispatch`], [`record_handler_duration`],
//! [`set_breaker_state`], etc.) — those look up the global registry
//! that was installed by [`register`] inside `observability::init`. A
//! call before `register` is a no-op (the helpers tolerate the missing
//! registry so unit tests don't panic).

use std::sync::OnceLock;

use opentelemetry::KeyValue;
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

/// Sub-phase E: process-global registry installed by [`register`] in
/// `observability::init`. Every dispatch point calls into the
/// module-level helpers below; if no registry has been installed (early
/// startup, unit-test path) the helpers are no-ops.
static GLOBAL: OnceLock<LifedMetrics> = OnceLock::new();

/// Build the metric registry against the global meter. Call this once
/// during `observability::init`. Per OpenTelemetry semantics, instruments
/// are cheap to construct repeatedly — but holding them on `VigilGuard`
/// keeps allocations to once per daemon. Sub-phase E additionally
/// publishes the registry to a process-global slot so dispatch-point
/// helpers can record without dependency-injection plumbing.
pub fn register() -> LifedMetrics {
    let meter: Meter = global::meter("lifed");
    let metrics = LifedMetrics {
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
    };
    // Build a clone-bag for the global slot. `LifedMetrics` is
    // composed of cheaply-cloneable instrument handles; we re-build
    // each instrument against the meter so the Counter/Histogram/Gauge
    // types match. Skipping the OnceLock set on contention is OK — a
    // double-init can only happen if `init` is called twice in the
    // same process (unit-test edge case), in which case the first
    // installation wins and subsequent series increments still flow
    // through the live meter.
    let global_view = build_global_view(&meter);
    let _ = GLOBAL.set(global_view);
    metrics
}

fn build_global_view(meter: &Meter) -> LifedMetrics {
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

/// Record one substrate dispatch. Counts by `(substrate, namespace,
/// method, status)`. Status is `"ok"` or `"err"`. Spec C₂ §9.3.
pub fn record_dispatch(substrate: &str, namespace: &str, method: &str, status: &str) {
    if let Some(m) = GLOBAL.get() {
        m.dispatch_count.add(
            1,
            &[
                KeyValue::new("substrate", substrate.to_string()),
                KeyValue::new("namespace", namespace.to_string()),
                KeyValue::new("method", method.to_string()),
                KeyValue::new("status", status.to_string()),
            ],
        );
    }
}

/// Record dispatch latency in milliseconds. Spec C₂ §9.3.
pub fn record_dispatch_duration_ms(substrate: &str, namespace: &str, method: &str, ms: u64) {
    if let Some(m) = GLOBAL.get() {
        m.dispatch_duration_ms.record(
            ms,
            &[
                KeyValue::new("substrate", substrate.to_string()),
                KeyValue::new("namespace", namespace.to_string()),
                KeyValue::new("method", method.to_string()),
            ],
        );
    }
}

/// Record handler latency in milliseconds. Spec C₂ §9.3.
pub fn record_handler_duration_ms(namespace: &str, method: &str, ms: u64) {
    if let Some(m) = GLOBAL.get() {
        m.handler_duration_ms.record(
            ms,
            &[
                KeyValue::new("namespace", namespace.to_string()),
                KeyValue::new("method", method.to_string()),
            ],
        );
    }
}

/// Set the gauge for active session count by tier (e.g. "Tier-2" / "Tier-3").
pub fn set_session_active(count: i64, tier: &str) {
    if let Some(m) = GLOBAL.get() {
        m.session_active
            .record(count, &[KeyValue::new("tier", tier.to_string())]);
    }
}

/// Bump the session-created counter. Spec C₂ §9.3.
pub fn record_session_created(tier: &str) {
    if let Some(m) = GLOBAL.get() {
        m.session_created
            .add(1, &[KeyValue::new("tier", tier.to_string())]);
    }
}

/// Bump the session-destroyed counter. Spec C₂ §9.3.
pub fn record_session_destroyed(tier: &str) {
    if let Some(m) = GLOBAL.get() {
        m.session_destroyed
            .add(1, &[KeyValue::new("tier", tier.to_string())]);
    }
}

/// Record the cold-start replay duration. Spec C₂ §9.3.
pub fn record_replay_seconds(seconds: f64) {
    if let Some(m) = GLOBAL.get() {
        m.session_replay_seconds.record(seconds, &[]);
    }
}

/// Set the routing-cache size gauge. Spec C₂ §9.3.
pub fn set_cache_size(size: i64) {
    if let Some(m) = GLOBAL.get() {
        m.cache_size.record(size, &[]);
    }
}

/// Bump the cache-eviction counter (label by reason — `idle` / `lru` /
/// `revoked`). Spec C₂ §9.3.
pub fn record_cache_eviction(reason: &str) {
    if let Some(m) = GLOBAL.get() {
        m.cache_evictions
            .add(1, &[KeyValue::new("reason", reason.to_string())]);
    }
}

/// Set the breaker-state gauge for `substrate`. Spec C₂ §9.3.
pub fn set_breaker_state(substrate: &str, state_value: i64) {
    if let Some(m) = GLOBAL.get() {
        m.breaker_state.record(
            state_value,
            &[KeyValue::new("substrate", substrate.to_string())],
        );
    }
}

/// Set the inflight-semaphore gauge for `substrate`. Spec C₂ §9.3.
pub fn set_semaphore_inflight(substrate: &str, inflight: i64) {
    if let Some(m) = GLOBAL.get() {
        m.semaphore_inflight.record(
            inflight,
            &[KeyValue::new("substrate", substrate.to_string())],
        );
    }
}

/// Bump the slow-stream counter (label by attachment kind). Spec C₂ §9.3.
pub fn record_slow_stream(attachment_kind: &str) {
    if let Some(m) = GLOBAL.get() {
        m.slow_stream_total.add(
            1,
            &[KeyValue::new(
                "attachment_kind",
                attachment_kind.to_string(),
            )],
        );
    }
}

/// Set the saga-inflight gauge by saga kind. Spec C₂ §9.3.
pub fn set_saga_inflight(kind: &str, count: i64) {
    if let Some(m) = GLOBAL.get() {
        m.saga_inflight
            .record(count, &[KeyValue::new("kind", kind.to_string())]);
    }
}

/// Bump the saga-completed counter. `outcome` is `succeeded` /
/// `compensated` / `failed`. Spec C₂ §9.3.
pub fn record_saga_completed(kind: &str, outcome: &str) {
    if let Some(m) = GLOBAL.get() {
        m.saga_completed.add(
            1,
            &[
                KeyValue::new("kind", kind.to_string()),
                KeyValue::new("outcome", outcome.to_string()),
            ],
        );
    }
}

/// Bump the saga-compensation-failed counter. Spec C₂ §9.3.
pub fn record_saga_compensation_failed(kind: &str, step: &str) {
    if let Some(m) = GLOBAL.get() {
        m.saga_compensation_failed.add(
            1,
            &[
                KeyValue::new("kind", kind.to_string()),
                KeyValue::new("step", step.to_string()),
            ],
        );
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

    #[test]
    fn dispatch_helpers_tolerate_missing_registry() {
        // Without `register` having been called the helpers must
        // silently drop the increment. This protects unit tests that
        // exercise dispatch paths without observability::init.
        record_dispatch("arcan", "agent", "create_session", "ok");
        record_dispatch_duration_ms("lago", "events", "read", 5);
        record_handler_duration_ms("agent", "send_message", 10);
        set_breaker_state("arcan", 0);
        set_semaphore_inflight("haima", 4);
        record_slow_stream("agent_event_stream");
        record_session_created("Tier-2");
        record_session_destroyed("Tier-2");
        set_session_active(1, "Tier-2");
        record_replay_seconds(0.5);
        set_cache_size(10);
        record_cache_eviction("idle");
        set_saga_inflight("create_session", 1);
        record_saga_completed("create_session", "succeeded");
        record_saga_compensation_failed("create_session", "bind_wallet");
    }
}
