//! Configuration for Vigil telemetry initialization.
//!
//! Supports construction from explicit values or environment variable overrides.

use std::env;

/// OTLP transport protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OtlpProtocol {
    #[default]
    Grpc,
    Http,
}

/// Log output format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
}

/// Vigil configuration — controls telemetry pipeline setup.
///
/// When constructed with [`VigConfig::default()`], all fields use sensible defaults.
/// Environment variables override programmatic values when [`VigConfig::from_env()`]
/// or [`VigConfig::with_env_overrides()`] is used.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VigConfig {
    /// Service identity for OTel resource (e.g. "arcan", "lagod", "autonomicd").
    pub service_name: String,

    /// OTLP collector endpoint (e.g. "http://localhost:4317").
    /// If `None`, no OTel exporter is configured — Vigil degrades to structured logging only.
    pub otlp_endpoint: Option<String>,

    /// Transport protocol for OTLP export.
    pub otlp_protocol: OtlpProtocol,

    /// Additional headers for OTLP export (e.g. Langfuse `Authorization: Basic <base64>`).
    pub otlp_headers: Vec<(String, String)>,

    /// Log output format.
    pub log_format: LogFormat,

    /// Trace sampling ratio (0.0 = none, 1.0 = all).
    pub sampling_ratio: f64,

    /// Whether to capture prompt/completion content in spans (privacy-sensitive).
    pub capture_content: bool,
}

impl Default for VigConfig {
    fn default() -> Self {
        Self {
            service_name: "vigil".to_string(),
            otlp_endpoint: None,
            otlp_protocol: OtlpProtocol::default(),
            otlp_headers: Vec::new(),
            log_format: LogFormat::default(),
            sampling_ratio: 1.0,
            capture_content: false,
        }
    }
}

impl VigConfig {
    /// Create a config for a named service with sensible defaults.
    pub fn for_service(name: impl Into<String>) -> Self {
        Self {
            service_name: name.into(),
            ..Default::default()
        }
    }

    /// Build config purely from environment variables.
    pub fn from_env() -> Self {
        Self::default().with_env_overrides()
    }

    /// Apply environment variable overrides on top of programmatic values.
    ///
    /// Supported env vars:
    /// - `OTEL_EXPORTER_OTLP_ENDPOINT` → `otlp_endpoint`
    /// - `OTEL_EXPORTER_OTLP_PROTOCOL` → `otlp_protocol` ("grpc" or "http/protobuf")
    /// - `OTEL_EXPORTER_OTLP_HEADERS` → `otlp_headers` (comma-separated `key=value` pairs)
    /// - `OTEL_SERVICE_NAME` → `service_name`
    /// - `VIGIL_LOG_FORMAT` → `log_format` ("json" or "pretty")
    /// - `VIGIL_CAPTURE_CONTENT` → `capture_content` ("true" or "1")
    /// - `VIGIL_SAMPLING_RATIO` → `sampling_ratio` (float 0.0..=1.0)
    pub fn with_env_overrides(mut self) -> Self {
        if let Ok(endpoint) = env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            if !endpoint.is_empty() {
                self.otlp_endpoint = Some(endpoint);
            }
        }

        // Standard OTel protocol env var: "grpc" or "http/protobuf"
        if let Ok(protocol) = env::var("OTEL_EXPORTER_OTLP_PROTOCOL") {
            match protocol.to_lowercase().as_str() {
                "grpc" => self.otlp_protocol = OtlpProtocol::Grpc,
                "http/protobuf" | "http" => self.otlp_protocol = OtlpProtocol::Http,
                _ => {} // ignore unknown values
            }
        }

        if let Ok(headers_str) = env::var("OTEL_EXPORTER_OTLP_HEADERS") {
            self.otlp_headers = parse_headers(&headers_str);
        }

        if let Ok(name) = env::var("OTEL_SERVICE_NAME") {
            if !name.is_empty() {
                self.service_name = name;
            }
        }

        if let Ok(fmt) = env::var("VIGIL_LOG_FORMAT") {
            match fmt.to_lowercase().as_str() {
                "json" => self.log_format = LogFormat::Json,
                "pretty" => self.log_format = LogFormat::Pretty,
                _ => {} // ignore unknown values
            }
        }

        if let Ok(capture) = env::var("VIGIL_CAPTURE_CONTENT") {
            self.capture_content = matches!(capture.as_str(), "true" | "1" | "yes");
        }

        if let Ok(ratio) = env::var("VIGIL_SAMPLING_RATIO") {
            if let Ok(r) = ratio.parse::<f64>() {
                self.sampling_ratio = r.clamp(0.0, 1.0);
            }
        }

        self
    }
}

/// Parse `key=value,key2=value2` header strings into pairs.
fn parse_headers(s: &str) -> Vec<(String, String)> {
    s.split(',')
        .filter_map(|pair| {
            let pair = pair.trim();
            if pair.is_empty() {
                return None;
            }
            let mut parts = pair.splitn(2, '=');
            let key = parts.next()?.trim().to_string();
            let value = parts.next()?.trim().to_string();
            if key.is_empty() {
                return None;
            }
            Some((key, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_no_endpoint() {
        let cfg = VigConfig::default();
        assert!(cfg.otlp_endpoint.is_none());
        assert_eq!(cfg.sampling_ratio, 1.0);
        assert!(!cfg.capture_content);
        assert_eq!(cfg.log_format, LogFormat::Pretty);
        assert_eq!(cfg.otlp_protocol, OtlpProtocol::Grpc);
    }

    #[test]
    fn for_service_sets_name() {
        let cfg = VigConfig::for_service("arcan");
        assert_eq!(cfg.service_name, "arcan");
    }

    #[test]
    fn parse_headers_works() {
        let result = parse_headers("Authorization=Basic abc123,X-Custom=value");
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            ("Authorization".to_string(), "Basic abc123".to_string())
        );
        assert_eq!(result[1], ("X-Custom".to_string(), "value".to_string()));
    }

    #[test]
    fn parse_headers_handles_empty() {
        assert!(parse_headers("").is_empty());
        assert!(parse_headers("  ").is_empty());
        assert!(parse_headers(",,,").is_empty());
    }

    #[test]
    fn parse_headers_handles_missing_value() {
        let result = parse_headers("key_only");
        assert!(result.is_empty());
    }

    // Env var tests are #[ignore] because env vars are process-global and
    // race under parallel test execution. Run with:
    //   cargo test -- --ignored --test-threads=1
    #[test]
    #[ignore]
    fn env_overrides_all_fields() {
        // SAFETY: Rust 2024 requires unsafe for set_var/remove_var.
        unsafe {
            env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://test:4317");
            env::set_var("OTEL_SERVICE_NAME", "vigil-test-svc");
            env::set_var("VIGIL_LOG_FORMAT", "json");
            env::set_var("VIGIL_CAPTURE_CONTENT", "true");
            env::set_var("VIGIL_SAMPLING_RATIO", "0.5");
        }

        let cfg = VigConfig::default().with_env_overrides();
        assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://test:4317"));
        assert_eq!(cfg.service_name, "vigil-test-svc");
        assert_eq!(cfg.log_format, LogFormat::Json);
        assert!(cfg.capture_content);
        assert!((cfg.sampling_ratio - 0.5).abs() < 0.001);

        unsafe {
            env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
            env::remove_var("OTEL_SERVICE_NAME");
            env::remove_var("VIGIL_LOG_FORMAT");
            env::remove_var("VIGIL_CAPTURE_CONTENT");
            env::remove_var("VIGIL_SAMPLING_RATIO");
        }
    }

    #[test]
    #[ignore]
    fn sampling_ratio_clamped() {
        unsafe {
            env::set_var("VIGIL_SAMPLING_RATIO", "5.0");
        }
        let cfg = VigConfig::default().with_env_overrides();
        assert!((cfg.sampling_ratio - 1.0).abs() < 0.001);

        unsafe {
            env::set_var("VIGIL_SAMPLING_RATIO", "-1.0");
        }
        let cfg = VigConfig::default().with_env_overrides();
        assert!(cfg.sampling_ratio.abs() < 0.001);

        unsafe {
            env::remove_var("VIGIL_SAMPLING_RATIO");
        }
    }

    #[test]
    fn config_builder_pattern() {
        let cfg = VigConfig {
            service_name: "test".to_string(),
            otlp_endpoint: Some("http://localhost:4317".to_string()),
            otlp_protocol: OtlpProtocol::Http,
            otlp_headers: vec![("Authorization".to_string(), "Bearer token".to_string())],
            log_format: LogFormat::Json,
            sampling_ratio: 0.5,
            capture_content: true,
        };
        assert_eq!(cfg.service_name, "test");
        assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://localhost:4317"));
        assert_eq!(cfg.otlp_protocol, OtlpProtocol::Http);
        assert_eq!(cfg.otlp_headers.len(), 1);
        assert_eq!(cfg.log_format, LogFormat::Json);
        assert!((cfg.sampling_ratio - 0.5).abs() < 0.001);
        assert!(cfg.capture_content);
    }
}
