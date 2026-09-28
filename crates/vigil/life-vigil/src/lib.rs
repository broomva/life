//! Vigil — observability foundation for the Life Agent OS.
//!
//! Provides a unified telemetry pipeline combining structured logging,
//! OpenTelemetry distributed tracing, and metrics collection with
//! GenAI semantic conventions.
//!
//! # Usage
//!
//! ```no_run
//! use life_vigil::{VigConfig, init_telemetry};
//!
//! # fn main() -> Result<(), life_vigil::VigError> {
//! let config = VigConfig::for_service("arcan").with_env_overrides();
//! let _guard = init_telemetry(config)?;
//!
//! // All tracing macros now emit structured logs and (if configured) OTel spans.
//! tracing::info!("Agent OS started");
//! # Ok(())
//! # }
//! ```
//!
//! If no OTLP endpoint is configured, Vigil degrades gracefully to
//! structured logging only via `tracing-subscriber`.

pub mod config;
pub mod envelope;
pub mod jsonl;
pub mod ledger;
pub mod metrics;
pub mod pricing;
pub mod semconv;
pub mod spans;
pub mod tokens;

/// Stream-aware observability — broadcast/mpsc lag, drain rate, and saturation
/// metrics (BRO-1322). Re-exported so `life-vigil` is the single observability
/// import surface; the implementation lives in the dependency-light
/// [`life_stream_metrics`] crate that the substrate primitives depend on.
pub mod stream {
    pub use life_stream_metrics::{
        MeasuredReceiver, MeasuredSender, StreamMetrics, measured_channel, measured_channel_with,
    };
}
pub use life_stream_metrics::{
    MeasuredReceiver, MeasuredSender, StreamMetrics, measured_channel, measured_channel_with,
};

pub use config::{LogFormat, OtlpProtocol, VigConfig};
pub use envelope::{CircuitState, CostSource, LlmRequestEnvelope, LlmResponseEconomics};
pub use jsonl::{JsonlWriter, LlmCallRecord};
pub use ledger::{
    Attribution, ExogeneityCheck, ExogeneityHook, ForkError, ForkEvent, ForkSample, ForkVariable,
    LedgerEvent, LedgerEventType, NonAttributiveReason, OutcomeDistribution, PearsonExogeneityHook,
    ReplayerIndependence, RuntimeIdentity, VariableKind, VersionProbeEvent, VersionStability,
    pearson_correlation,
};
pub use metrics::GenAiMetrics;
pub use pricing::{ModelPricing, PRICING_SNAPSHOT, estimate_cost, lookup_pricing};
pub use tokens::estimate_tokens;

use std::sync::OnceLock;

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_otlp::WithHttpConfig as _;
use opentelemetry_otlp::WithTonicConfig as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

static VIGIL_CONFIG: OnceLock<VigConfig> = OnceLock::new();

/// Check if LangSmith enrichment is enabled.
pub fn langsmith_enrichment_enabled() -> bool {
    VIGIL_CONFIG
        .get()
        .map(|c| c.langsmith_enrichment)
        .unwrap_or(false)
}

/// Errors during telemetry initialization.
#[derive(Debug, thiserror::Error)]
pub enum VigError {
    #[error("failed to build OTLP span exporter: {0}")]
    SpanExporter(String),

    #[error("failed to build OTLP metric exporter: {0}")]
    MetricExporter(String),

    #[error("failed to initialize tracing subscriber: {0}")]
    Subscriber(String),
}

/// Guard that flushes and shuts down telemetry providers on drop.
///
/// Hold this in your `main()` for the lifetime of the application.
pub struct VigGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl VigGuard {
    /// Whether OTLP export is active. `false` means logging only: either no
    /// endpoint was configured, or exporter init failed and was degraded.
    pub fn is_exporting(&self) -> bool {
        self.tracer_provider.is_some() || self.meter_provider.is_some()
    }
}

impl Drop for VigGuard {
    fn drop(&mut self) {
        if let Some(ref tp) = self.tracer_provider {
            let _ = tp.shutdown();
        }
        if let Some(ref mp) = self.meter_provider {
            let _ = mp.shutdown();
        }
    }
}

/// Initialize the Vigil telemetry pipeline.
///
/// Sets up:
/// 1. `tracing-subscriber` with `EnvFilter` + formatted output (pretty or JSON)
/// 2. `tracing-opentelemetry` layer bridging to OTel SDK (if endpoint configured)
/// 3. OTLP exporter for traces (if endpoint configured)
/// 4. OTLP exporter for metrics (if endpoint configured)
///
/// Returns a [`VigGuard`] that flushes telemetry on drop.
///
/// If no OTLP endpoint is set, only structured logging is configured.
///
/// OTLP export is advisory: if an exporter cannot be built (bad endpoint,
/// missing TLS support, …) this logs a warning and falls back to logging
/// only, so a telemetry misconfiguration never stops a daemon from booting
/// (BRO-2642). Only a subscriber install failure is returned as an error.
pub fn init_telemetry(config: VigConfig) -> Result<VigGuard, VigError> {
    let _ = VIGIL_CONFIG.set(config.clone());

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let Some(ref endpoint) = config.otlp_endpoint else {
        return init_logging_only(&config, env_filter);
    };

    match build_providers(&config, endpoint) {
        Ok((tracer_provider, meter_provider)) => {
            init_with_otel(&config, tracer_provider, meter_provider, env_filter)
        }
        Err(e) => {
            let guard = init_logging_only(&config, env_filter)?;
            tracing::warn!(
                error = %e,
                "OTLP exporter init failed; continuing with logging only (no telemetry export)"
            );
            Ok(guard)
        }
    }
}

/// Build both OTLP providers before touching any global state, so a failure
/// in either leaves nothing half-installed.
fn build_providers(
    config: &VigConfig,
    endpoint: &str,
) -> Result<(SdkTracerProvider, SdkMeterProvider), VigError> {
    // Resource::builder() automatically includes EnvResourceDetector,
    // which reads OTEL_RESOURCE_ATTRIBUTES (e.g. langsmith.project.name=arcan).
    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .build();

    let tracer_provider = build_tracer_provider(config, endpoint, resource.clone())?;
    let meter_provider = match build_meter_provider(config, endpoint, resource) {
        Ok(mp) => mp,
        Err(e) => {
            let _ = tracer_provider.shutdown();
            return Err(e);
        }
    };
    Ok((tracer_provider, meter_provider))
}

/// Initialize with full OTel pipeline.
fn init_with_otel(
    config: &VigConfig,
    tracer_provider: SdkTracerProvider,
    meter_provider: SdkMeterProvider,
    env_filter: EnvFilter,
) -> Result<VigGuard, VigError> {
    global::set_tracer_provider(tracer_provider.clone());
    global::set_meter_provider(meter_provider.clone());

    // Create OTel tracing layer with INFO filter to exclude debug-level
    // infrastructure spans (e.g. lago.journal.append) from OTLP export.
    let tracer = tracer_provider.tracer(config.service_name.clone());
    let otel_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_filter(tracing_subscriber::filter::LevelFilter::INFO);

    // Build subscriber with OTel layer + fmt layer
    let registry = tracing_subscriber::registry().with(otel_layer);

    match config.log_format {
        LogFormat::Json => {
            let fmt_layer = tracing_subscriber::fmt::layer()
                .json()
                .with_filter(env_filter);
            registry.with(fmt_layer).try_init().map_err(
                |e: tracing_subscriber::util::TryInitError| VigError::Subscriber(e.to_string()),
            )?;
        }
        LogFormat::Pretty => {
            let fmt_layer = tracing_subscriber::fmt::layer().with_filter(env_filter);
            registry.with(fmt_layer).try_init().map_err(
                |e: tracing_subscriber::util::TryInitError| VigError::Subscriber(e.to_string()),
            )?;
        }
    }

    Ok(VigGuard {
        tracer_provider: Some(tracer_provider),
        meter_provider: Some(meter_provider),
    })
}

/// Initialize with logging only (no OTel export).
fn init_logging_only(config: &VigConfig, env_filter: EnvFilter) -> Result<VigGuard, VigError> {
    match config.log_format {
        LogFormat::Json => {
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_filter(env_filter),
                )
                .try_init()
                .map_err(|e: tracing_subscriber::util::TryInitError| {
                    VigError::Subscriber(e.to_string())
                })?;
        }
        LogFormat::Pretty => {
            tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer().with_filter(env_filter))
                .try_init()
                .map_err(|e: tracing_subscriber::util::TryInitError| {
                    VigError::Subscriber(e.to_string())
                })?;
        }
    }

    Ok(VigGuard {
        tracer_provider: None,
        meter_provider: None,
    })
}

/// TLS config for a gRPC OTLP endpoint: `Some` (with trust roots) for https.
///
/// The exporter's own https default is a `ClientTlsConfig` with no trust
/// roots, which builds fine and then fails every handshake at export time.
pub fn grpc_tls_config(
    endpoint: &str,
) -> Option<opentelemetry_otlp::tonic_types::transport::ClientTlsConfig> {
    let is_https = endpoint
        .get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"));
    is_https.then(|| {
        opentelemetry_otlp::tonic_types::transport::ClientTlsConfig::new().with_webpki_roots()
    })
}

/// Build an OTLP tracer provider.
fn build_tracer_provider(
    config: &VigConfig,
    endpoint: &str,
    resource: Resource,
) -> Result<SdkTracerProvider, VigError> {
    let exporter = match config.otlp_protocol {
        OtlpProtocol::Grpc => {
            let mut builder = opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint);

            if let Some(tls) = grpc_tls_config(endpoint) {
                builder = builder.with_tls_config(tls);
            }

            if !config.otlp_headers.is_empty() {
                let mut metadata = tonic::metadata::MetadataMap::new();
                for (key, value) in &config.otlp_headers {
                    if let (Ok(k), Ok(v)) = (
                        key.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>(),
                        value.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>(),
                    ) {
                        metadata.insert(k, v);
                    }
                }
                builder = builder.with_metadata(metadata);
            }

            builder
                .build()
                .map_err(|e| VigError::SpanExporter(e.to_string()))?
        }
        OtlpProtocol::Http => {
            let mut builder = opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_endpoint(endpoint);

            if !config.otlp_headers.is_empty() {
                let mut headers = std::collections::HashMap::new();
                for (key, value) in &config.otlp_headers {
                    headers.insert(key.clone(), value.clone());
                }
                builder = builder.with_headers(headers);
            }

            builder
                .build()
                .map_err(|e| VigError::SpanExporter(e.to_string()))?
        }
    };

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    Ok(provider)
}

/// Build an OTLP meter provider.
fn build_meter_provider(
    config: &VigConfig,
    endpoint: &str,
    resource: Resource,
) -> Result<SdkMeterProvider, VigError> {
    let exporter = match config.otlp_protocol {
        OtlpProtocol::Grpc => {
            let mut builder = opentelemetry_otlp::MetricExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint);

            if let Some(tls) = grpc_tls_config(endpoint) {
                builder = builder.with_tls_config(tls);
            }

            if !config.otlp_headers.is_empty() {
                let mut metadata = tonic::metadata::MetadataMap::new();
                for (key, value) in &config.otlp_headers {
                    if let (Ok(k), Ok(v)) = (
                        key.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>(),
                        value.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>(),
                    ) {
                        metadata.insert(k, v);
                    }
                }
                builder = builder.with_metadata(metadata);
            }

            builder
                .build()
                .map_err(|e| VigError::MetricExporter(e.to_string()))?
        }
        OtlpProtocol::Http => {
            let mut builder = opentelemetry_otlp::MetricExporter::builder()
                .with_http()
                .with_endpoint(endpoint);

            if !config.otlp_headers.is_empty() {
                let mut headers = std::collections::HashMap::new();
                for (key, value) in &config.otlp_headers {
                    headers.insert(key.clone(), value.clone());
                }
                builder = builder.with_headers(headers);
            }

            builder
                .build()
                .map_err(|e| VigError::MetricExporter(e.to_string()))?
        }
    };

    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter)
        .with_resource(resource)
        .build();

    Ok(provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_telemetry_no_endpoint_succeeds() {
        let config = VigConfig::for_service("test");
        let result = init_telemetry(config);
        match result {
            Ok(guard) => {
                assert!(guard.tracer_provider.is_none());
                assert!(guard.meter_provider.is_none());
            }
            Err(VigError::Subscriber(_)) => {
                // Acceptable: global subscriber already set by another test
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn vig_error_display() {
        let e = VigError::SpanExporter("test error".to_string());
        assert!(e.to_string().contains("test error"));

        let e = VigError::MetricExporter("metric error".to_string());
        assert!(e.to_string().contains("metric error"));

        let e = VigError::Subscriber("sub error".to_string());
        assert!(e.to_string().contains("sub error"));
    }

    const HTTPS_ENDPOINT: &str = "https://us.cloud.langfuse.com/api/public/otel";

    fn config_for(protocol: OtlpProtocol) -> VigConfig {
        VigConfig {
            otlp_protocol: protocol,
            ..VigConfig::for_service("test")
        }
    }

    /// BRO-2642: with opentelemetry-otlp >= 0.30 and no `tls-*` feature, an
    /// https endpoint fails exporter build, which panicked lagod at boot.
    #[tokio::test]
    async fn exporters_build_against_https_endpoint() {
        for protocol in [OtlpProtocol::Grpc, OtlpProtocol::Http] {
            let config = config_for(protocol);
            let resource = Resource::builder().with_service_name("test").build();

            let tp = build_tracer_provider(&config, HTTPS_ENDPOINT, resource.clone())
                .unwrap_or_else(|e| panic!("{protocol:?} span exporter over https: {e}"));
            let mp = build_meter_provider(&config, HTTPS_ENDPOINT, resource)
                .unwrap_or_else(|e| panic!("{protocol:?} metric exporter over https: {e}"));
            let guard = VigGuard {
                tracer_provider: Some(tp),
                meter_provider: Some(mp),
            };
            assert!(guard.is_exporting());
        }
    }

    #[test]
    fn grpc_tls_config_only_for_https() {
        assert!(grpc_tls_config(HTTPS_ENDPOINT).is_some());
        assert!(grpc_tls_config("HTTPS://collector:4317").is_some());
        assert!(grpc_tls_config("http://localhost:4317").is_none());
        assert!(grpc_tls_config("").is_none());
    }

    #[tokio::test]
    async fn build_providers_reports_invalid_endpoint() {
        let config = config_for(OtlpProtocol::Grpc);
        assert!(matches!(
            build_providers(&config, "not a uri"),
            Err(VigError::SpanExporter(_))
        ));
    }

    #[test]
    fn vig_guard_drop_is_safe() {
        let guard = VigGuard {
            tracer_provider: None,
            meter_provider: None,
        };
        drop(guard);
    }
}
