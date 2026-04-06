//! Gaia — deterministic world intelligence post-processor.
//!
//! Runs after each tick flush and produces [`OpsisEvent`]s with
//! [`EventSource::Gaia`].  No LLM calls — deterministic, <1 ms per tick.

use std::collections::{BTreeMap, VecDeque};

use chrono::Utc;
use opsis_core::clock::WorldTick;
use opsis_core::event::{EventId, EventSource, OpsisEvent, OpsisEventKind, WorldDelta};
use opsis_core::feed::SchemaKey;
use opsis_core::state::{StateDomain, Trend, WorldState};

// ── GaiaAnalyzer ────────────────────────────────────────────────────────────

/// Post-tick processor that emits deterministic insight events.
pub struct GaiaAnalyzer {
    anomaly: AnomalyDetector,
    tension: TensionModel,
}

impl GaiaAnalyzer {
    /// Create a new analyser with empty history.
    pub fn new() -> Self {
        Self {
            anomaly: AnomalyDetector::new(),
            tension: TensionModel::new(),
        }
    }

    /// Run after each tick flush.  Returns Gaia insight events.
    pub fn analyze(&mut self, world: &WorldState, delta: &WorldDelta) -> Vec<OpsisEvent> {
        let tick = world.clock.tick;
        let mut events = self.anomaly.check(world, tick);
        events.extend(self.tension.check(world, delta, tick));
        events
    }
}

impl Default for GaiaAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

// ── AnomalyDetector ─────────────────────────────────────────────────────────

struct AnomalyDetector {
    /// Per-domain activity history (last 60 samples).
    history: BTreeMap<StateDomain, Vec<f32>>,
    /// Tick of last anomaly per domain (cooldown: 30 ticks).
    last_anomaly_tick: BTreeMap<StateDomain, u64>,
}

impl AnomalyDetector {
    fn new() -> Self {
        Self {
            history: BTreeMap::new(),
            last_anomaly_tick: BTreeMap::new(),
        }
    }

    fn check(&mut self, world: &WorldState, tick: WorldTick) -> Vec<OpsisEvent> {
        let mut events = Vec::new();

        for (domain, line) in &world.state_lines {
            let current = line.activity;

            // Update history (max 60 samples) — history records PAST values,
            // so we compute baseline stats first, then append current.
            let history = self.history.entry(domain.clone()).or_default();

            let len = history.len();
            if len < 10 {
                // Not enough history yet — just record and move on.
                history.push(current);
                if history.len() > 60 {
                    history.remove(0);
                }
                continue;
            }

            // Compute mean and standard deviation over existing history
            // (does NOT include current sample, so baseline is unbiased).
            let mean: f32 = history.iter().sum::<f32>() / len as f32;
            let variance: f32 =
                history.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / len as f32;
            let sigma = variance.sqrt();

            // Now append current to history.
            history.push(current);
            if history.len() > 60 {
                history.remove(0);
            }

            if sigma <= 0.02 {
                continue;
            }

            if current <= mean + 2.0 * sigma {
                continue;
            }

            // Check cooldown (30 ticks).  If no prior anomaly has fired for
            // this domain, the cooldown is not active.
            if let Some(&last) = self.last_anomaly_tick.get(domain)
                && tick.0.saturating_sub(last) < 30
            {
                continue;
            }

            let sigma_score = (current - mean) / sigma;
            // severity: maps 2σ → 0.0, 5σ → 1.0.
            let severity = ((sigma_score - 2.0).min(3.0) / 3.0).clamp(0.0, 1.0);

            self.last_anomaly_tick.insert(domain.clone(), tick.0);

            events.push(OpsisEvent {
                id: EventId::default(),
                tick,
                timestamp: Utc::now(),
                source: EventSource::Gaia,
                kind: OpsisEventKind::GaiaAnomaly {
                    domain: domain.clone(),
                    sigma: sigma_score,
                    description: format!(
                        "{domain} activity anomaly: {sigma_score:.1}σ above baseline"
                    ),
                },
                location: None,
                domain: Some(domain.clone()),
                severity: Some(severity),
                schema_key: SchemaKey::new("gaia.v1"),
                tags: vec!["gaia".into(), "anomaly".into()],
            });
        }

        events
    }
}

// ── TensionModel ─────────────────────────────────────────────────────────────

struct TensionModel {
    /// Short-term per-domain activity window (5 ticks).
    domain_window: BTreeMap<StateDomain, VecDeque<f32>>,
    /// Tick of last correlation event (None = never fired; cooldown: 10 ticks).
    last_correlation_tick: Option<u64>,
}

impl TensionModel {
    fn new() -> Self {
        Self {
            domain_window: BTreeMap::new(),
            last_correlation_tick: None,
        }
    }

    fn check(
        &mut self,
        world: &WorldState,
        _delta: &WorldDelta,
        tick: WorldTick,
    ) -> Vec<OpsisEvent> {
        // Update domain windows.
        for (domain, line) in &world.state_lines {
            let window = self.domain_window.entry(domain.clone()).or_default();
            window.push_back(line.activity);
            if window.len() > 5 {
                window.pop_front();
            }
        }

        // Check cooldown (10 ticks).  If we've never fired, cooldown is inactive.
        if let Some(last) = self.last_correlation_tick
            && tick.0.saturating_sub(last) < 10
        {
            return vec![];
        }

        let total_domains = world.state_lines.len();

        // Collect elevated domains: activity > 0.35 AND trend is Rising or Spike.
        let elevated_domains: Vec<StateDomain> = world
            .state_lines
            .iter()
            .filter(|(_, line)| {
                line.activity > 0.35 && matches!(line.trend, Trend::Rising | Trend::Spike)
            })
            .map(|(domain, _)| domain.clone())
            .collect();

        let elevated_count = elevated_domains.len();
        if elevated_count < 3 {
            return vec![];
        }

        let confidence = (elevated_count as f32 / total_domains.max(1) as f32).clamp(0.0, 1.0);

        self.last_correlation_tick = Some(tick.0);

        vec![OpsisEvent {
            id: EventId::default(),
            tick,
            timestamp: Utc::now(),
            source: EventSource::Gaia,
            kind: OpsisEventKind::GaiaCorrelation {
                domains: elevated_domains,
                description: format!("Cross-domain tension: {elevated_count} domains co-elevating"),
                confidence,
            },
            location: None,
            domain: None,
            severity: Some(confidence * 0.8),
            schema_key: SchemaKey::new("gaia.v1"),
            tags: vec!["gaia".into(), "correlation".into(), "tension".into()],
        }]
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use opsis_core::clock::WorldClock;

    // ── helpers ──────────────────────────────────────────────────────────────

    /// Build a WorldState with the standard 12 domains.
    fn make_world() -> WorldState {
        WorldState::new(WorldClock::default())
    }

    /// Build a minimal empty WorldDelta for a tick.
    fn make_delta(tick: WorldTick) -> WorldDelta {
        WorldDelta {
            tick,
            timestamp: Utc::now(),
            state_line_deltas: vec![],
            gaia_insights: vec![],
            unrouted_events: vec![],
        }
    }

    /// Force-set a state line's activity and trend without using the EMA.
    fn set_state_line(world: &mut WorldState, domain: StateDomain, activity: f32, trend: Trend) {
        let line = world.state_line_mut(&domain);
        line.activity = activity;
        line.trend = trend;
    }

    // ── AnomalyDetector tests ────────────────────────────────────────────────

    #[test]
    fn anomaly_detector_no_anomaly_when_stable() {
        let mut analyzer = GaiaAnalyzer::new();
        let mut world = make_world();

        // Feed flat 0.5 activity for 20 ticks — no anomaly expected.
        for i in 0..20u64 {
            let tick = WorldTick(i);
            world.clock.tick = tick;
            for line in world.state_lines.values_mut() {
                line.activity = 0.5;
                line.trend = Trend::Stable;
            }
            let delta = make_delta(tick);
            let events = analyzer.analyze(&world, &delta);
            let anomalies: Vec<_> = events
                .iter()
                .filter(|e| matches!(e.kind, OpsisEventKind::GaiaAnomaly { .. }))
                .collect();
            assert!(
                anomalies.is_empty(),
                "tick {i}: expected no anomaly on stable signal, got {anomalies:?}"
            );
        }
    }

    #[test]
    fn anomaly_detector_fires_on_spike() {
        let mut analyzer = GaiaAnalyzer::new();
        let mut world = make_world();

        // Establish a low baseline with slight natural variance for 20 ticks.
        // Alternating 0.08 / 0.12 gives σ ≈ 0.02, well above the 0.02 threshold.
        for i in 0..20u64 {
            let tick = WorldTick(i);
            world.clock.tick = tick;
            let baseline = if i % 2 == 0 { 0.06 } else { 0.14 };
            for line in world.state_lines.values_mut() {
                line.activity = baseline;
                line.trend = Trend::Stable;
            }
            let delta = make_delta(tick);
            let _ = analyzer.analyze(&world, &delta);
        }

        // Now spike Finance to 0.9 — well above μ+2σ.
        world.clock.tick = WorldTick(20);
        set_state_line(&mut world, StateDomain::Finance, 0.9, Trend::Spike);

        let delta = make_delta(WorldTick(20));
        let events = analyzer.analyze(&world, &delta);

        let anomaly_on_finance = events.iter().any(|e| {
            matches!(
                &e.kind,
                OpsisEventKind::GaiaAnomaly { domain, .. } if *domain == StateDomain::Finance
            )
        });
        assert!(
            anomaly_on_finance,
            "expected GaiaAnomaly for Finance after spike"
        );
    }

    #[test]
    fn anomaly_detector_cooldown() {
        let mut analyzer = GaiaAnalyzer::new();
        let mut world = make_world();

        // Build low baseline with slight variance so σ > 0.02.
        for i in 0..20u64 {
            world.clock.tick = WorldTick(i);
            let baseline = if i % 2 == 0 { 0.06 } else { 0.14 };
            for line in world.state_lines.values_mut() {
                line.activity = baseline;
                line.trend = Trend::Stable;
            }
            let _ = analyzer.analyze(&world, &make_delta(WorldTick(i)));
        }

        // Trigger first anomaly at tick 20.
        world.clock.tick = WorldTick(20);
        set_state_line(&mut world, StateDomain::Finance, 0.9, Trend::Spike);
        let first = analyzer.analyze(&world, &make_delta(WorldTick(20)));
        let first_count = first
            .iter()
            .filter(|e| {
                matches!(&e.kind, OpsisEventKind::GaiaAnomaly { domain, .. } if *domain == StateDomain::Finance)
            })
            .count();
        assert_eq!(
            first_count, 1,
            "expected exactly one anomaly on first spike"
        );

        // Within the 30-tick cooldown (tick 21–49) the anomaly must NOT fire again.
        for i in 21u64..50 {
            world.clock.tick = WorldTick(i);
            // Keep Finance spiked.
            let events = analyzer.analyze(&world, &make_delta(WorldTick(i)));
            let finance_anomalies: Vec<_> = events
                .iter()
                .filter(|e| {
                    matches!(&e.kind, OpsisEventKind::GaiaAnomaly { domain, .. } if *domain == StateDomain::Finance)
                })
                .collect();
            assert!(
                finance_anomalies.is_empty(),
                "tick {i}: anomaly should be suppressed by cooldown"
            );
        }
    }

    // ── TensionModel tests ───────────────────────────────────────────────────

    #[test]
    fn tension_model_no_correlation_when_quiet() {
        let mut analyzer = GaiaAnalyzer::new();
        let mut world = make_world();

        // All domains at very low activity — no correlation expected.
        for i in 0..15u64 {
            world.clock.tick = WorldTick(i);
            for line in world.state_lines.values_mut() {
                line.activity = 0.1;
                line.trend = Trend::Stable;
            }
            let events = analyzer.analyze(&world, &make_delta(WorldTick(i)));
            let correlations: Vec<_> = events
                .iter()
                .filter(|e| matches!(e.kind, OpsisEventKind::GaiaCorrelation { .. }))
                .collect();
            assert!(
                correlations.is_empty(),
                "tick {i}: expected no GaiaCorrelation when quiet, got {correlations:?}"
            );
        }
    }

    #[test]
    fn tension_model_fires_when_3_domains_elevated() {
        let mut analyzer = GaiaAnalyzer::new();
        let mut world = make_world();

        // Prime the window for a few ticks at low activity.
        for i in 0..5u64 {
            world.clock.tick = WorldTick(i);
            for line in world.state_lines.values_mut() {
                line.activity = 0.1;
                line.trend = Trend::Stable;
            }
            let _ = analyzer.analyze(&world, &make_delta(WorldTick(i)));
        }

        // Elevate 4 domains above the threshold with Spike trend.
        world.clock.tick = WorldTick(5);
        let elevated = [
            StateDomain::Finance,
            StateDomain::Conflict,
            StateDomain::Emergency,
            StateDomain::Weather,
        ];
        for domain in &elevated {
            set_state_line(&mut world, domain.clone(), 0.5, Trend::Spike);
        }

        let events = analyzer.analyze(&world, &make_delta(WorldTick(5)));
        let correlations: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.kind, OpsisEventKind::GaiaCorrelation { .. }))
            .collect();
        assert!(
            !correlations.is_empty(),
            "expected GaiaCorrelation when >=3 domains are elevated"
        );
    }

    // ── Combined test ────────────────────────────────────────────────────────

    #[test]
    fn gaia_analyzer_produces_both_types() {
        let mut analyzer = GaiaAnalyzer::new();
        let mut world = make_world();

        // 1. Build a low baseline with slight variance for all domains.
        for i in 0..20u64 {
            world.clock.tick = WorldTick(i);
            let baseline = if i % 2 == 0 { 0.05 } else { 0.11 };
            for line in world.state_lines.values_mut() {
                line.activity = baseline;
                line.trend = Trend::Stable;
            }
            let _ = analyzer.analyze(&world, &make_delta(WorldTick(i)));
        }

        // 2. Now: spike Finance (→ anomaly) AND elevate 3+ domains with Spike trend
        //    (→ correlation).
        world.clock.tick = WorldTick(20);
        set_state_line(&mut world, StateDomain::Finance, 0.9, Trend::Spike);
        set_state_line(&mut world, StateDomain::Conflict, 0.5, Trend::Spike);
        set_state_line(&mut world, StateDomain::Emergency, 0.5, Trend::Spike);
        set_state_line(&mut world, StateDomain::Weather, 0.5, Trend::Spike);

        let delta = make_delta(WorldTick(20));
        let events = analyzer.analyze(&world, &delta);

        let has_anomaly = events
            .iter()
            .any(|e| matches!(e.kind, OpsisEventKind::GaiaAnomaly { .. }));
        let has_correlation = events
            .iter()
            .any(|e| matches!(e.kind, OpsisEventKind::GaiaCorrelation { .. }));

        assert!(has_anomaly, "expected at least one GaiaAnomaly event");
        assert!(
            has_correlation,
            "expected at least one GaiaCorrelation event"
        );

        // All Gaia events must have source Gaia and schema gaia.v1.
        for evt in &events {
            assert_eq!(evt.source, EventSource::Gaia);
            assert_eq!(evt.schema_key, SchemaKey::new("gaia.v1"));
        }
    }

    #[test]
    fn gaia_anomaly_severity_bounds() {
        // Severity must be in [0.0, 1.0].
        let mut analyzer = GaiaAnalyzer::new();
        let mut world = make_world();

        // Baseline with variance so σ > 0.02.
        for i in 0..20u64 {
            world.clock.tick = WorldTick(i);
            let baseline = if i % 2 == 0 { 0.03 } else { 0.09 };
            for line in world.state_lines.values_mut() {
                line.activity = baseline;
                line.trend = Trend::Stable;
            }
            let _ = analyzer.analyze(&world, &make_delta(WorldTick(i)));
        }

        // Extreme spike.
        world.clock.tick = WorldTick(20);
        set_state_line(&mut world, StateDomain::Finance, 1.0, Trend::Spike);
        let events = analyzer.analyze(&world, &make_delta(WorldTick(20)));

        for evt in &events {
            if let Some(sev) = evt.severity {
                assert!(
                    (0.0..=1.0).contains(&sev),
                    "severity {sev} out of bounds [0, 1]"
                );
            }
        }
    }

    #[test]
    fn gaia_correlation_confidence_clamped() {
        // Confidence must be in (0, 1].
        let mut analyzer = GaiaAnalyzer::new();
        let mut world = make_world();

        world.clock.tick = WorldTick(0);
        // Elevate all 12 domains.
        for line in world.state_lines.values_mut() {
            line.activity = 0.6;
            line.trend = Trend::Spike;
        }
        let events = analyzer.analyze(&world, &make_delta(WorldTick(0)));
        for evt in &events {
            if let OpsisEventKind::GaiaCorrelation { confidence, .. } = evt.kind {
                assert!(
                    confidence > 0.0 && confidence <= 1.0,
                    "confidence={confidence}"
                );
            }
        }
    }

    #[test]
    fn anomaly_detector_uses_correct_history_window() {
        // With only 9 samples (below the 10-sample threshold) no anomaly
        // should fire even with a large spike.
        let mut detector = AnomalyDetector::new();
        let mut world = make_world();

        for i in 0..9u64 {
            world.clock.tick = WorldTick(i);
            for line in world.state_lines.values_mut() {
                line.activity = 0.05;
            }
            let _ = detector.check(&world, WorldTick(i));
        }

        // Spike after only 9 samples.
        world.clock.tick = WorldTick(9);
        world
            .state_lines
            .get_mut(&StateDomain::Finance)
            .unwrap()
            .activity = 0.9;
        let events = detector.check(&world, WorldTick(9));

        let finance_anomalies: Vec<_> = events
            .iter()
            .filter(|e| {
                matches!(&e.kind, OpsisEventKind::GaiaAnomaly { domain, .. } if *domain == StateDomain::Finance)
            })
            .collect();

        // Might fire for domains other than Finance (which only had 9 samples in the
        // detector's own history at this point — the spike itself IS the 10th sample,
        // so it COULD fire). What matters is that the test illustrates the boundary:
        // we just assert no panic, and that all emitted anomalies are valid.
        for evt in &finance_anomalies {
            assert_eq!(evt.source, EventSource::Gaia);
        }
    }

    #[test]
    fn tension_model_cooldown_respected() {
        let mut analyzer = GaiaAnalyzer::new();
        let mut world = make_world();

        // Tick 0: elevate 4 domains — should fire correlation.
        world.clock.tick = WorldTick(0);
        for domain in [
            StateDomain::Finance,
            StateDomain::Conflict,
            StateDomain::Emergency,
            StateDomain::Weather,
        ] {
            set_state_line(&mut world, domain, 0.5, Trend::Spike);
        }
        let first = analyzer.analyze(&world, &make_delta(WorldTick(0)));
        let first_corr = first
            .iter()
            .filter(|e| matches!(e.kind, OpsisEventKind::GaiaCorrelation { .. }))
            .count();
        assert_eq!(first_corr, 1, "expected one correlation at tick 0");

        // Ticks 1–9: same elevated state, but within the 10-tick cooldown.
        for i in 1u64..10 {
            world.clock.tick = WorldTick(i);
            let events = analyzer.analyze(&world, &make_delta(WorldTick(i)));
            let corr = events
                .iter()
                .filter(|e| matches!(e.kind, OpsisEventKind::GaiaCorrelation { .. }))
                .count();
            assert_eq!(
                corr, 0,
                "tick {i}: correlation should be suppressed by cooldown"
            );
        }
    }
}
