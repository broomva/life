//! Vigil wiring: tracing subscriber + canonical kernel.* metrics.
//!
//! `init` sets up the tracing subscriber and OTLP exporter according to the
//! daemon's `VigilConfig`. `KernelMetrics` registers three histograms/counters
//! that every RPC handler emits into:
//!
//! - `kernel.vm.lifecycle` (counter, labeled by `action`) — VM lifecycle
//!   transitions (create, destroy, hibernate, resume, snapshot, fork).
//! - `kernel.dispatch.duration` (histogram, ms, labeled by `tool_name`) —
//!   wall-clock dispatch latency.
//! - `kernel.egress.bytes` (counter, labeled by `vm_id`) — per-VM egress
//!   accounting; populated when the engine emits `KernelEgressRecorded`.
//!
//! # Egress accounting deferral
//!
//! `kernel.egress.bytes` is registered here for metric-name consistency but is
//! not incremented on the RPC path. Egress is naturally accounted via a Lago
//! event subscriber (`KernelEgressRecorded`) rather than the synchronous RPC
//! path. The counter is populated in BRO-903 when the event-store subscription
//! is wired. Until then `record_egress` is a no-call documented hook.

use std::time::Duration;

use life_vigil::{VigConfig, VigError, VigGuard, init_telemetry};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};

use crate::config::{VigilConfig, VigilExporter};
use crate::error::{LifedError, LifedResult};

/// Initialise Vigil tracing + metrics for the daemon. Returns a guard that
/// flushes spans + metrics on drop. Hold it for the process lifetime.
pub fn init(cfg: &VigilConfig) -> LifedResult<VigGuard> {
    let mut vig = VigConfig::for_service("lifed");
    match &cfg.exporter {
        VigilExporter::Console => {
            // VigConfig defaults emit to stdout/journald via the tracing
            // subscriber's fmt layer; no OTLP endpoint set means metrics
            // stay in-process.
        }
        VigilExporter::Otlp { endpoint } => {
            vig.otlp_endpoint = Some(endpoint.clone());
        }
    }
    // `init_telemetry` also respects OTEL_EXPORTER_OTLP_ENDPOINT and
    // VIGIL_LOG_FORMAT env vars via the VigConfig — callers can always
    // override programmatic values.
    init_telemetry(vig).map_err(|e: VigError| LifedError::Config(format!("vigil init: {e}")))
}

/// Canonical kernel.* metric handles. Cheap to clone — each field is an
/// `Arc`-backed OTel instrument handle. Registers each instrument once with
/// the global meter and stores the handle.
#[derive(Clone)]
pub struct KernelMetrics {
    vm_lifecycle: Counter<u64>,
    dispatch_duration: Histogram<f64>,
    egress_bytes: Counter<u64>,
}

impl KernelMetrics {
    /// Register the three canonical metrics on the global meter.
    ///
    /// Must be called after [`init`] so that the global meter provider is set
    /// (when an OTLP endpoint is configured). In no-endpoint mode the metrics
    /// remain in-process no-ops but are still constructable.
    pub fn register() -> Self {
        let meter: Meter = opentelemetry::global::meter("lifed");
        Self {
            vm_lifecycle: meter
                .u64_counter("kernel.vm.lifecycle")
                .with_description("VM lifecycle transitions")
                .build(),
            dispatch_duration: meter
                .f64_histogram("kernel.dispatch.duration")
                .with_description("Dispatch wall-clock duration (ms)")
                .with_unit("ms")
                .build(),
            egress_bytes: meter
                .u64_counter("kernel.egress.bytes")
                .with_description("Per-VM egress bytes recorded")
                .with_unit("By")
                .build(),
        }
    }

    /// Record a lifecycle transition. `action` is one of
    /// `"create" | "destroy" | "hibernate" | "resume" | "snapshot" | "fork"`.
    pub fn record_lifecycle(&self, action: &'static str) {
        self.vm_lifecycle.add(1, &[KeyValue::new("action", action)]);
    }

    /// Record a dispatch latency sample, in milliseconds.
    pub fn observe_dispatch(&self, elapsed: Duration, tool_name: String) {
        self.dispatch_duration.record(
            elapsed.as_secs_f64() * 1000.0,
            &[KeyValue::new("tool_name", tool_name)],
        );
    }

    /// Record per-VM egress bytes.
    ///
    /// NOTE: Not currently called on the RPC path. Populated by the Lago
    /// event-store subscriber in BRO-903 when `KernelEgressRecorded` fires.
    pub fn record_egress(&self, vm_id: String, bytes: u64) {
        self.egress_bytes
            .add(bytes, &[KeyValue::new("vm_id", vm_id)]);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//                                   Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{VigilConfig, VigilExporter};

    /// `KernelMetrics::register()` must not panic, even when no OTel exporter
    /// is configured (no-op global meter provider is used instead).
    #[test]
    fn kernel_metrics_register_does_not_panic() {
        let _metrics = KernelMetrics::register();
    }

    /// `record_lifecycle` must not panic for each lifecycle action label.
    #[test]
    fn record_lifecycle_all_actions_do_not_panic() {
        let metrics = KernelMetrics::register();
        for action in &[
            "create",
            "destroy",
            "hibernate",
            "resume",
            "snapshot",
            "fork",
        ] {
            metrics.record_lifecycle(action);
        }
    }

    /// `observe_dispatch` must not panic for various durations and tool names.
    #[test]
    fn observe_dispatch_does_not_panic() {
        let metrics = KernelMetrics::register();
        metrics.observe_dispatch(Duration::from_millis(42), "shell".to_string());
        metrics.observe_dispatch(Duration::from_secs(5), "read_file".to_string());
        metrics.observe_dispatch(Duration::from_nanos(100), "noop".to_string());
    }

    /// `record_egress` must not panic.
    #[test]
    fn record_egress_does_not_panic() {
        let metrics = KernelMetrics::register();
        metrics.record_egress("vm-abc".to_string(), 1024);
    }

    /// `KernelMetrics` must be `Clone` (required by `LifeKernelService<E>: Clone`).
    #[test]
    fn kernel_metrics_is_clone() {
        let metrics = KernelMetrics::register();
        let _clone = metrics.clone();
    }

    /// `observability::init` with Console exporter must succeed.
    /// In tests the global subscriber may already be set by another test; the
    /// `Subscriber` error variant is acceptable — it means Vigil was already
    /// initialised in this test process.
    #[test]
    fn init_console_exporter_succeeds_or_already_init() {
        let cfg = VigilConfig {
            exporter: VigilExporter::Console,
            level: "info".to_string(),
        };
        match init(&cfg) {
            Ok(_guard) => {} // guard dropped here — flush on drop is safe
            Err(LifedError::Config(msg))
                if msg.contains("already") || msg.contains("Subscriber") =>
            {
                // Global subscriber already set by another test — acceptable.
            }
            Err(e) => panic!("unexpected observability::init error: {e:?}"),
        }
    }
}
