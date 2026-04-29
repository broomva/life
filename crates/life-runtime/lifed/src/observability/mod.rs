//! Vigil exporter init + observability submodules.
//!
//! Sub-phase D5 splits the previous single-file `observability.rs` into a
//! folder with three submodules:
//! - `metrics`            — registers the metric series per Spec C₂ §9.3.
//! - `spans`              — span-attribute helpers (rpc.system, rpc.method, life.user_id...).
//! - `trace_propagation`  — tower middleware that extracts the W3C
//!   `traceparent` header and stamps it onto the per-request span.
//!
//! Sub-phase E swaps the tracing-subscriber shim for `life_vigil`'s OTLP
//! pipeline when `cfg.vigil.otlp_endpoint` is set. The OTLP exporter
//! ships traces + metrics over gRPC (`opentelemetry-otlp` crate) so the
//! browser→lifegw→lifed→substrate trace tree threads end-to-end. When
//! the endpoint is unset, lifed degrades to a `tracing-subscriber` shim
//! exactly as before, plus the in-process metric registry stays alive
//! against the global no-op meter.

pub mod handler_metrics;
pub mod metrics;
pub mod spans;
pub mod trace_propagation;

pub use handler_metrics::HandlerMetricsLayer;
pub use trace_propagation::TracePropagationLayer;

use crate::config::VigilConfig;
use crate::error::{LifedError, LifedResult};

/// RAII guard returned by `init` — drops on daemon exit and flushes
/// anything buffered. Sub-phase E wires the canonical `life_vigil`
/// pipeline: when `cfg.vigil.otlp_endpoint` is configured the guard
/// holds the [`life_vigil::VigGuard`] so trace + metric providers stay
/// alive for the daemon's lifetime; otherwise lifed falls back to
/// `tracing-subscriber` only.
pub struct VigilGuard {
    /// Sub-phase E: the underlying vigil guard when an OTLP endpoint is
    /// wired. `None` when lifed runs in logging-only mode.
    _vigil: Option<life_vigil::VigGuard>,
    /// Registered metric handles. Held inside the guard so the meter
    /// provider stays alive for the daemon's lifetime even in
    /// logging-only mode (the no-op global meter still produces valid
    /// instruments).
    _metrics: metrics::LifedMetrics,
}

pub fn init(cfg: &VigilConfig) -> LifedResult<VigilGuard> {
    // Sub-phase E: prefer the canonical life_vigil OTLP pipeline when
    // the operator has configured an endpoint. life_vigil installs the
    // global tracer + meter providers AND a `tracing-subscriber` layer
    // bridging tracing macros into OTel. Per Spec C₂ §9.4.
    let vigil_guard = match cfg.otlp_endpoint.as_deref() {
        Some(endpoint) if !endpoint.is_empty() => {
            let vig_cfg = life_vigil::VigConfig {
                service_name: "lifed".to_string(),
                otlp_endpoint: Some(endpoint.to_string()),
                sampling_ratio: cfg.trace_sample_ratio,
                ..Default::default()
            };
            match life_vigil::init_telemetry(vig_cfg) {
                Ok(g) => Some(g),
                Err(e) => {
                    // Logging-only fall-through: the operator's OTLP
                    // collector might be temporarily unreachable; do
                    // not refuse to boot.
                    tracing::warn!(
                        endpoint = endpoint,
                        error = %e,
                        "vigil OTLP init failed — falling back to logging-only",
                    );
                    None
                }
            }
        }
        _ => None,
    };

    if vigil_guard.is_none() {
        // Logging-only path. tracing_subscriber::set_global_default is
        // tolerant of being called twice (returns Err on the second
        // call) — match the prior shim's leniency.
        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .with_target(true)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new("lifed=info,tower=warn")
                }),
            )
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    }

    // Sub-phase E: install the canonical W3C TraceContext propagator so
    // outbound substrate calls inject `traceparent` headers using the
    // OTel context lifed received from lifegw. life_vigil already sets
    // the global TracerProvider; this is the missing piece on the
    // propagation side.
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    let metrics = metrics::register();
    Ok(VigilGuard {
        _vigil: vigil_guard,
        _metrics: metrics,
    })
}

#[allow(dead_code)]
fn _coerce_vigil_error(e: life_vigil::VigError) -> LifedError {
    LifedError::Config(format!("vigil init: {e}"))
}
