//! Vigil exporter init.
//!
//! Sub-phase A: stdout fallback only. Real OTLP exporter wires in D5.

use crate::config::VigilConfig;
use crate::error::LifedResult;

/// RAII guard returned by `init` — drops on daemon exit and flushes anything
/// buffered. Sub-phase A is a no-op; sub-phase D (D5) wires the real OTLP
/// flush.
pub struct VigilGuard;

pub fn init(_cfg: &VigilConfig) -> LifedResult<VigilGuard> {
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_target(true)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("lifed=info,tower=warn")),
        )
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
    Ok(VigilGuard)
}
