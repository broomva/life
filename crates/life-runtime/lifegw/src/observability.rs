//! Vigil exporter init.
//!
//! Sub-phase A: stdout fallback only. Real OTLP exporter wires in
//! Sub-phase D (Spec C₃ §9.4).

use crate::config::ObservabilityConfig;
use crate::error::LifegwResult;

/// RAII guard returned by `init` — drops on daemon exit and flushes anything
/// buffered. Sub-phase A is a no-op; Sub-phase D wires the real OTLP flush.
pub struct VigilGuard;

pub fn init(_cfg: &ObservabilityConfig) -> LifegwResult<VigilGuard> {
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_target(true)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("lifegw=info,tower=warn")),
        )
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
    Ok(VigilGuard)
}
