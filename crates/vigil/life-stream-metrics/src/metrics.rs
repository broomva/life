//! [`StreamMetrics`] — the OTel instrument bundle for a single broadcast /
//! mpsc channel.
//!
//! Every metric is labelled with the channel name (baked into the bundle at
//! construction) plus, where meaningful, a `consumer_id` and `event_type`.
//! The instruments follow the `stream.broadcast.*` naming so a single Grafana
//! dashboard template works across every instrumented channel.

use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};
use opentelemetry::{KeyValue, global};

/// Metric: items published to a channel since boot.
pub const METRIC_PUBLISHED: &str = "stream.broadcast.published_total";
/// Metric: items each consumer drained.
pub const METRIC_CONSUMED: &str = "stream.broadcast.consumed_total";
/// Metric: count of `RecvError::Lagged` events per consumer.
pub const METRIC_LAGGED: &str = "stream.broadcast.lagged_total";
/// Metric: sum of `skipped` reported by each `Lagged` per consumer.
pub const METRIC_SKIPPED: &str = "stream.broadcast.skipped_messages_total";
/// Metric: `max_lag / capacity` snapshot (0..=1).
pub const METRIC_SATURATION: &str = "stream.broadcast.buffer_saturation";
/// Metric: active subscribers right now.
pub const METRIC_CONSUMER_COUNT: &str = "stream.broadcast.consumer_count";
/// Metric: wall-clock latency from publish to consumed (seconds).
pub const METRIC_DELTA_LATENCY: &str = "stream.broadcast.delta_latency_seconds";

/// Attribute key: the channel name (e.g. `lago.journal.stream`).
pub const ATTR_CHANNEL: &str = "channel";
/// Attribute key: a per-channel consumer identifier.
pub const ATTR_CONSUMER: &str = "consumer_id";
/// Attribute key: a low-cardinality event-type tag.
pub const ATTR_EVENT_TYPE: &str = "event_type";

/// The OTel meter name every stream instrument is created under.
pub const METER_NAME: &str = "life-stream-metrics";

/// A pre-created bundle of OTel instruments for one stream channel.
///
/// Cheap to clone — every field is an OTel instrument handle backed by an
/// `Arc`, so cloning shares the underlying instrument. Recording is a no-op
/// when no meter provider is installed (Vigil's graceful-degradation path),
/// so callers can instrument unconditionally without a feature flag.
#[derive(Clone)]
pub struct StreamMetrics {
    channel: String,
    published_total: Counter<u64>,
    consumed_total: Counter<u64>,
    lagged_total: Counter<u64>,
    skipped_messages_total: Counter<u64>,
    buffer_saturation: Gauge<f64>,
    consumer_count: Gauge<u64>,
    delta_latency_seconds: Histogram<f64>,
}

impl StreamMetrics {
    /// Build a bundle for `channel` from the global meter provider.
    ///
    /// Intended for production: call this once the channel is created (which,
    /// for the substrate daemons, is after `life_vigil::init_telemetry` has
    /// installed the global meter provider, so the instruments bind to the
    /// real exporter rather than the no-op).
    pub fn new(channel: impl Into<String>) -> Self {
        let meter = global::meter(METER_NAME);
        Self::from_meter(&meter, channel)
    }

    /// Build a bundle for `channel` from a specific meter.
    ///
    /// Used by tests (to bind to an in-memory reader) and by hosts that hold
    /// their own meter.
    pub fn from_meter(meter: &Meter, channel: impl Into<String>) -> Self {
        let channel = channel.into();
        let published_total = meter
            .u64_counter(METRIC_PUBLISHED)
            .with_description("Items published to the channel since boot")
            .build();
        let consumed_total = meter
            .u64_counter(METRIC_CONSUMED)
            .with_description("Items drained by each consumer")
            .build();
        let lagged_total = meter
            .u64_counter(METRIC_LAGGED)
            .with_description("Count of RecvError::Lagged events per consumer")
            .build();
        let skipped_messages_total = meter
            .u64_counter(METRIC_SKIPPED)
            .with_description("Sum of skipped messages reported by each Lagged")
            .build();
        let buffer_saturation = meter
            .f64_gauge(METRIC_SATURATION)
            .with_description("max_lag / capacity snapshot (0..=1)")
            .build();
        let consumer_count = meter
            .u64_gauge(METRIC_CONSUMER_COUNT)
            .with_description("Active subscribers on the channel")
            .build();
        let delta_latency_seconds = meter
            .f64_histogram(METRIC_DELTA_LATENCY)
            .with_description("Wall-clock latency from publish to consumed")
            .with_unit("s")
            .with_boundaries(vec![
                0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0,
            ])
            .build();

        Self {
            channel,
            published_total,
            consumed_total,
            lagged_total,
            skipped_messages_total,
            buffer_saturation,
            consumer_count,
            delta_latency_seconds,
        }
    }

    /// The channel name this bundle is labelled with.
    pub fn channel(&self) -> &str {
        &self.channel
    }

    fn channel_attr(&self) -> KeyValue {
        KeyValue::new(ATTR_CHANNEL, self.channel.clone())
    }

    /// Record one item published. `event_type` is an optional low-cardinality
    /// tag (e.g. an `EventKind` variant name); `None` omits the label.
    pub fn on_published(&self, event_type: Option<&str>) {
        let mut attrs = vec![self.channel_attr()];
        if let Some(et) = event_type {
            attrs.push(KeyValue::new(ATTR_EVENT_TYPE, et.to_string()));
        }
        self.published_total.add(1, &attrs);
    }

    /// Record one item drained by `consumer_id`, plus its publish→consume
    /// latency when available.
    pub fn on_consumed(&self, consumer_id: &str, event_type: Option<&str>) {
        let mut attrs = vec![
            self.channel_attr(),
            KeyValue::new(ATTR_CONSUMER, consumer_id.to_string()),
        ];
        if let Some(et) = event_type {
            attrs.push(KeyValue::new(ATTR_EVENT_TYPE, et.to_string()));
        }
        self.consumed_total.add(1, &attrs);
    }

    /// Record a `RecvError::Lagged(skipped)` for `consumer_id`: one lag event
    /// and `skipped` dropped messages.
    pub fn on_lagged(&self, consumer_id: &str, skipped: u64) {
        let attrs = [
            self.channel_attr(),
            KeyValue::new(ATTR_CONSUMER, consumer_id.to_string()),
        ];
        self.lagged_total.add(1, &attrs);
        self.skipped_messages_total.add(skipped, &attrs);
    }

    /// Snapshot the channel's saturation: `len / capacity`, clamped to 0..=1.
    /// A `capacity` of 0 records 0.0.
    pub fn set_saturation(&self, len: usize, capacity: usize) {
        let ratio = if capacity == 0 {
            0.0
        } else {
            (len as f64 / capacity as f64).clamp(0.0, 1.0)
        };
        self.buffer_saturation.record(ratio, &[self.channel_attr()]);
    }

    /// Snapshot the number of active subscribers.
    pub fn set_consumer_count(&self, count: usize) {
        self.consumer_count
            .record(count as u64, &[self.channel_attr()]);
    }

    /// Record publish→consume latency in seconds.
    pub fn record_delta_latency(&self, event_type: Option<&str>, seconds: f64) {
        if !seconds.is_finite() || seconds < 0.0 {
            return;
        }
        let mut attrs = vec![self.channel_attr()];
        if let Some(et) = event_type {
            attrs.push(KeyValue::new(ATTR_EVENT_TYPE, et.to_string()));
        }
        self.delta_latency_seconds.record(seconds, &attrs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_methods_are_noop_safe_without_a_provider() {
        // With no global meter provider installed, every record is a no-op —
        // callers can instrument unconditionally. This must not panic.
        let m = StreamMetrics::new("noop.channel");
        assert_eq!(m.channel(), "noop.channel");
        m.on_published(Some("UserMessage"));
        m.on_published(None);
        m.on_consumed("c#0", Some("UserMessage"));
        m.on_lagged("c#0", 12);
        m.set_saturation(5, 10);
        m.set_saturation(0, 0);
        m.set_consumer_count(3);
        m.record_delta_latency(Some("UserMessage"), 0.01);
        m.record_delta_latency(None, f64::NAN); // ignored
        m.record_delta_latency(None, -1.0); // ignored
    }

    #[test]
    fn saturation_clamps_to_unit_interval() {
        // Indirectly exercised: len > capacity must clamp to 1.0, not panic.
        let m = StreamMetrics::new("clamp.channel");
        m.set_saturation(20, 10);
        m.set_saturation(10, 10);
    }
}
