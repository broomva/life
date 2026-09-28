//! BRO-2642: a failed OTLP exporter init must not stop startup.
//!
//! Its own test binary so `init_telemetry` installs the global subscriber
//! exactly once in this process.

use life_vigil::{VigConfig, init_telemetry};

#[tokio::test]
async fn exporter_init_failure_degrades_to_logging_only() {
    let config = VigConfig {
        otlp_endpoint: Some("not a uri".to_string()),
        ..VigConfig::for_service("test")
    };

    let guard = init_telemetry(config).expect("exporter failure must not fail init");
    assert!(!guard.is_exporting());
    tracing::info!("logging still works after a degraded telemetry init");
}
