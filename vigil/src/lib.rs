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
pub mod metrics;
pub mod semconv;
pub mod spans;

pub use config::{LogFormat, OtlpProtocol, VigConfig};
pub use metrics::GenAiMetrics;

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
pub fn init_telemetry(config: VigConfig) -> Result<VigGuard, VigError> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if let Some(ref endpoint) = config.otlp_endpoint {
        init_with_otel(config.clone(), endpoint, env_filter)
    } else {
        init_logging_only(&config, env_filter)
    }
}

/// Initialize with full OTel pipeline.
fn init_with_otel(
    config: VigConfig,
    endpoint: &str,
    env_filter: EnvFilter,
) -> Result<VigGuard, VigError> {
    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .build();

    // Build tracer provider
    let tracer_provider = build_tracer_provider(&config, endpoint, resource.clone())?;
    global::set_tracer_provider(tracer_provider.clone());

    // Build meter provider
    let meter_provider = build_meter_provider(&config, endpoint, resource)?;
    global::set_meter_provider(meter_provider.clone());

    // Create OTel tracing layer
    let tracer = tracer_provider.tracer(config.service_name.clone());
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

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

    #[test]
    fn vig_guard_drop_is_safe() {
        let guard = VigGuard {
            tracer_provider: None,
            meter_provider: None,
        };
        drop(guard);
    }
}
