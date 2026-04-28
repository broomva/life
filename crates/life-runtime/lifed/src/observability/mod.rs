//! Vigil exporter init + observability submodules.
//!
//! Sub-phase D5 splits the previous single-file `observability.rs` into a
//! folder with three submodules:
//! - `metrics`            — registers the metric series per Spec C₂ §9.3.
//! - `spans`              — span-attribute helpers (rpc.system, rpc.method, life.user_id...).
//! - `trace_propagation`  — tower middleware that extracts the W3C
//!   `traceparent` header and stamps it onto the per-request span.
//!
//! `init` here remains a thin shim around `tracing_subscriber` for the
//! M5 default. Sub-phase E swaps in the OTLP exporter when the OTLP
//! endpoint is configured (Spec C₂ §9.1).

pub mod metrics;
pub mod spans;
pub mod trace_propagation;

pub use trace_propagation::TracePropagationLayer;

use crate::config::VigilConfig;
use crate::error::LifedResult;

/// RAII guard returned by `init` — drops on daemon exit and flushes
/// anything buffered. Sub-phase D wires the metric registry behind the
/// guard so it survives until shutdown.
pub struct VigilGuard {
    /// Registered metric handles. Held inside the guard so the meter
    /// provider stays alive for the daemon's lifetime.
    _metrics: metrics::LifedMetrics,
}

pub fn init(_cfg: &VigilConfig) -> LifedResult<VigilGuard> {
    // Sub-phase D5: thin tracing-subscriber shim retained from sub-phase A.
    // The OTLP wiring lands in sub-phase E once we pin the OTLP endpoint
    // configuration through `cfg.vigil.otlp_endpoint`.
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_target(true)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("lifed=info,tower=warn")),
        )
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
    let metrics = metrics::register();
    Ok(VigilGuard { _metrics: metrics })
}
