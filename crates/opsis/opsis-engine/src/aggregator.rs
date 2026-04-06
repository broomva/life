//! Tick aggregator — accumulates events between ticks and produces [`WorldDelta`].

use std::collections::BTreeMap;

use opsis_core::event::{OpsisEvent, StateLineDelta, WorldDelta};
use opsis_core::spatial::{GeoHotspot, GeoPoint};
use opsis_core::state::{StateDomain, WorldState};

/// Buffers events between ticks and flushes to produce a [`WorldDelta`].
pub struct TickAggregator {
    buffer: Vec<OpsisEvent>,
}

impl TickAggregator {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    /// Add an event to the tick buffer.
    pub fn push(&mut self, event: OpsisEvent) {
        self.buffer.push(event);
    }

    /// Drain the buffer: update state lines, build delta, clear.
    pub fn flush(&mut self, world: &mut WorldState) -> WorldDelta {
        let tick = world.clock.tick;

        // Group buffered events by domain, skipping events without a domain.
        let mut domain_events: BTreeMap<StateDomain, Vec<OpsisEvent>> = BTreeMap::new();
        for event in self.buffer.drain(..) {
            if let Some(ref domain) = event.domain {
                domain_events.entry(domain.clone()).or_default().push(event);
            }
            // Events without a domain are silently skipped from aggregation.
        }

        let mut state_line_deltas = Vec::new();

        // Update each domain that received events.
        for (domain, events) in &domain_events {
            let event_count = events.len() as f32;
            let severity_sum: f32 = events.iter().map(|e| e.severity.unwrap_or(0.0)).sum();
            let avg_severity = severity_sum / event_count.max(1.0);

            let line = world.state_line_mut(domain);
            line.update_activity(avg_severity, tick);

            let locations: Vec<GeoPoint> = events.iter().filter_map(|e| e.location).collect();
            line.hotspots = cluster_simple(&locations, 50.0);

            // Top-K events by severity (max 10).
            let mut top_events = events.clone();
            top_events.sort_by(|a, b| {
                let sev_b = b.severity.unwrap_or(0.0);
                let sev_a = a.severity.unwrap_or(0.0);
                sev_b
                    .partial_cmp(&sev_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            top_events.truncate(10);

            state_line_deltas.push(StateLineDelta {
                domain: domain.clone(),
                activity: line.activity,
                trend: line.trend,
                new_events: top_events,
                hotspots: line.hotspots.clone(),
            });
        }

        // Decay domains with no new events toward zero.
        for (domain, line) in &mut world.state_lines {
            if !domain_events.contains_key(domain) {
                line.update_activity(0.0, tick);
            }
        }

        WorldDelta {
            tick,
            timestamp: chrono::Utc::now(),
            state_line_deltas,
        }
    }
}

impl Default for TickAggregator {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple greedy clustering: group points within `eps_km` of each cluster seed.
fn cluster_simple(points: &[GeoPoint], eps_km: f64) -> Vec<GeoHotspot> {
    if points.is_empty() {
        return Vec::new();
    }

    let mut used = vec![false; points.len()];
    let mut hotspots = Vec::new();

    for i in 0..points.len() {
        if used[i] {
            continue;
        }
        used[i] = true;

        let mut cluster = vec![points[i]];
        for j in (i + 1)..points.len() {
            if !used[j] && points[i].distance_km(&points[j]) < eps_km {
                used[j] = true;
                cluster.push(points[j]);
            }
        }

        let count = cluster.len() as u32;
        let center = GeoPoint::new(
            cluster.iter().map(|p| p.lat).sum::<f64>() / count as f64,
            cluster.iter().map(|p| p.lon).sum::<f64>() / count as f64,
        );
        let radius_km = cluster
            .iter()
            .map(|p| center.distance_km(p))
            .fold(0.0_f64, f64::max) as f32;

        hotspots.push(GeoHotspot {
            center,
            radius_km,
            intensity: count as f32 / points.len().max(1) as f32,
            event_count: count,
        });
    }

    hotspots
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use opsis_core::clock::{WorldClock, WorldTick};
    use opsis_core::event::{EventId, EventSource, OpsisEventKind};
    use opsis_core::feed::{FeedSource, SchemaKey};

    fn make_event(domain: StateDomain, severity: f32, location: Option<GeoPoint>) -> OpsisEvent {
        OpsisEvent {
            id: EventId::default(),
            tick: WorldTick::zero(),
            timestamp: Utc::now(),
            source: EventSource::Feed(FeedSource::new("test")),
            kind: OpsisEventKind::WorldObservation {
                summary: "test".into(),
            },
            location,
            domain: Some(domain),
            severity: Some(severity),
            schema_key: SchemaKey::new("test.v1"),
            tags: vec![],
        }
    }

    #[test]
    fn flush_empty_buffer_produces_empty_delta() {
        let mut agg = TickAggregator::new();
        let mut world = WorldState::new(WorldClock::default());
        let delta = agg.flush(&mut world);
        assert!(delta.state_line_deltas.is_empty());
    }

    #[test]
    fn flush_updates_state_line_activity() {
        let mut agg = TickAggregator::new();
        let mut world = WorldState::new(WorldClock::default());

        agg.push(make_event(StateDomain::Emergency, 0.8, None));
        agg.push(make_event(StateDomain::Emergency, 0.6, None));

        let delta = agg.flush(&mut world);
        assert_eq!(delta.state_line_deltas.len(), 1);

        let line = &world.state_lines[&StateDomain::Emergency];
        assert!(line.activity > 0.0);
    }

    #[test]
    fn flush_top_k_limits_events() {
        let mut agg = TickAggregator::new();
        let mut world = WorldState::new(WorldClock::default());

        for i in 0..20 {
            agg.push(make_event(StateDomain::Weather, i as f32 / 20.0, None));
        }

        let delta = agg.flush(&mut world);
        assert_eq!(delta.state_line_deltas[0].new_events.len(), 10);
        // Highest severity first.
        assert!(
            delta.state_line_deltas[0].new_events[0]
                .severity
                .unwrap_or(0.0)
                > delta.state_line_deltas[0].new_events[9]
                    .severity
                    .unwrap_or(0.0)
        );
    }

    #[test]
    fn events_without_domain_are_skipped() {
        let mut agg = TickAggregator::new();
        let mut world = WorldState::new(WorldClock::default());

        // Push an event with no domain.
        let mut evt = make_event(StateDomain::Emergency, 0.5, None);
        evt.domain = None;
        agg.push(evt);

        let delta = agg.flush(&mut world);
        assert!(delta.state_line_deltas.is_empty());
    }

    #[test]
    fn cluster_simple_groups_nearby() {
        let points = vec![
            GeoPoint::new(4.0, -74.0),
            GeoPoint::new(4.001, -74.001),
            GeoPoint::new(40.0, -74.0), // Far away
        ];
        let clusters = cluster_simple(&points, 50.0);
        assert_eq!(clusters.len(), 2);
    }
}
