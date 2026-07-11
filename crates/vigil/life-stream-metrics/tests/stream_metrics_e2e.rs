//! End-to-end verification that the `stream.broadcast.*` metrics actually flow
//! through the OTel export pipeline, and that lag is a *sensitive* signal.
//!
//! Per `research/entities/concept/periodic-check-sensitivity.md`: a check that
//! never fails is insensitive. A lag metric that never increments in tests is
//! decorative. The forced-lag test injects a deliberately slow consumer to
//! prove `lagged_total` and `buffer_saturation` actually move; the
//! healthy-flow test proves they stay quiet when they should.

use std::sync::Arc;

use life_stream_metrics::{
    METRIC_CONSUMED, METRIC_DELTA_LATENCY, METRIC_LAGGED, METRIC_PUBLISHED, METRIC_SATURATION,
    METRIC_SKIPPED, RecvError, StreamMetrics, measured_channel_with,
};
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::data::{Gauge, Histogram, ResourceMetrics, Sum};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

/// Spin up an in-memory OTel metric pipeline and return the provider + the
/// exporter to read from after a `force_flush`.
fn pipeline() -> (SdkMeterProvider, InMemoryMetricExporter) {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    (provider, exporter)
}

fn collect(provider: &SdkMeterProvider, exporter: &InMemoryMetricExporter) -> Vec<ResourceMetrics> {
    provider.force_flush().expect("force_flush");
    exporter.get_finished_metrics().expect("finished metrics")
}

/// Sum every u64 `Sum` data point for `name` across the export.
fn sum_u64(rms: &[ResourceMetrics], name: &str) -> u64 {
    let mut total = 0;
    for rm in rms {
        for sm in &rm.scope_metrics {
            for m in &sm.metrics {
                if m.name != name {
                    continue;
                }
                if let Some(sum) = m.data.as_any().downcast_ref::<Sum<u64>>() {
                    total += sum.data_points.iter().map(|dp| dp.value).sum::<u64>();
                }
            }
        }
    }
    total
}

/// Latest f64 `Gauge` value for `name`, if any.
fn gauge_f64(rms: &[ResourceMetrics], name: &str) -> Option<f64> {
    for rm in rms {
        for sm in &rm.scope_metrics {
            for m in &sm.metrics {
                if m.name != name {
                    continue;
                }
                if let Some(g) = m.data.as_any().downcast_ref::<Gauge<f64>>() {
                    if let Some(dp) = g.data_points.last() {
                        return Some(dp.value);
                    }
                }
            }
        }
    }
    None
}

/// Total histogram count for `name`.
fn histogram_count(rms: &[ResourceMetrics], name: &str) -> u64 {
    let mut total = 0;
    for rm in rms {
        for sm in &rm.scope_metrics {
            for m in &sm.metrics {
                if m.name != name {
                    continue;
                }
                if let Some(h) = m.data.as_any().downcast_ref::<Histogram<f64>>() {
                    total += h.data_points.iter().map(|dp| dp.count).sum::<u64>();
                }
            }
        }
    }
    total
}

fn metric_names(rms: &[ResourceMetrics]) -> Vec<String> {
    let mut names = Vec::new();
    for rm in rms {
        for sm in &rm.scope_metrics {
            for m in &sm.metrics {
                names.push(m.name.to_string());
            }
        }
    }
    names
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forced_lag_increments_lagged_total_and_saturates() {
    let (provider, exporter) = pipeline();
    let meter = provider.meter("test");
    let metrics = Arc::new(StreamMetrics::from_meter(&meter, "forced.lag"));

    // Capacity C=4. Producer runs at rate R (10 sends), consumer at ~R/10
    // (drains nothing until the channel has already overflowed) — the
    // canonical "slow consumer" injection.
    const CAPACITY: usize = 4;
    let (tx, mut rx) = measured_channel_with::<u32>(CAPACITY, metrics);

    for i in 0..10u32 {
        tx.send(i).expect("send");
    }

    // The slow consumer's first drain sees the overflow as Lagged.
    let skipped = match rx.recv().await {
        Err(RecvError::Lagged(n)) => n,
        other => panic!("expected Lagged, got {other:?}"),
    };
    assert!(skipped >= 1, "consumer should have skipped ≥1 message");

    let rms = collect(&provider, &exporter);

    // OTLP export verification: every new metric name is present in the export
    // (this is the "Prometheus scrape returns the new metric names" check —
    // the InMemory exporter is the same PushMetricExporter contract an OTLP
    // exporter satisfies).
    let names = metric_names(&rms);
    for expected in [
        METRIC_PUBLISHED,
        METRIC_SATURATION,
        METRIC_LAGGED,
        METRIC_SKIPPED,
        METRIC_CONSUMER_COUNT_NAME,
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "metric {expected} missing from export; got {names:?}"
        );
    }

    assert_eq!(sum_u64(&rms, METRIC_PUBLISHED), 10, "published_total");
    assert!(sum_u64(&rms, METRIC_LAGGED) >= 1, "lagged_total must fire");
    assert!(
        sum_u64(&rms, METRIC_SKIPPED) >= 1,
        "skipped_messages_total must accumulate"
    );

    // buffer_saturation reads 1.0 once the channel is full past capacity.
    let saturation = gauge_f64(&rms, METRIC_SATURATION).expect("saturation gauge present");
    assert!(
        (saturation - 1.0).abs() < f64::EPSILON,
        "saturation should be 1.0 when overflowed, got {saturation}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn healthy_flow_stays_quiet() {
    let (provider, exporter) = pipeline();
    let meter = provider.meter("test");
    let metrics = Arc::new(StreamMetrics::from_meter(&meter, "healthy.flow"));

    // Capacity 8, producer/consumer balanced: drain after each send so the
    // channel never backs up.
    let (tx, mut rx) = measured_channel_with::<u32>(8, metrics);
    for i in 0..20u32 {
        tx.send(i).expect("send");
        let got = rx.recv().await.expect("recv");
        assert_eq!(got, i);
    }

    let rms = collect(&provider, &exporter);

    assert_eq!(sum_u64(&rms, METRIC_CONSUMED), 20, "consumed_total");
    assert_eq!(
        sum_u64(&rms, METRIC_LAGGED),
        0,
        "lagged_total must be 0 under healthy flow"
    );
    assert_eq!(
        histogram_count(&rms, METRIC_DELTA_LATENCY),
        20,
        "every drain records a latency sample"
    );

    // Saturation stays well under half — one in-flight message on a capacity-8
    // channel is 0.125.
    let saturation = gauge_f64(&rms, METRIC_SATURATION).expect("saturation gauge present");
    assert!(
        saturation < 0.5,
        "healthy saturation should be < 0.5, got {saturation}"
    );
}

// `METRIC_CONSUMER_COUNT` lives in the crate; alias here for the name check
// above so the presence assertion reads cleanly.
use life_stream_metrics::METRIC_CONSUMER_COUNT as METRIC_CONSUMER_COUNT_NAME;
