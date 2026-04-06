use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::clock::{WorldClock, WorldTick};
use crate::spatial::GeoHotspot;

/// A domain of world-state activity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum StateDomain {
    Emergency,
    Health,
    Finance,
    Trade,
    Conflict,
    Politics,
    Weather,
    Space,
    Ocean,
    Technology,
    Personal,
    Infrastructure,
    Custom(String),
}

impl StateDomain {
    /// Parse a domain name string into a StateDomain.
    pub fn from_name(name: &str) -> Self {
        match name {
            "Emergency" => Self::Emergency,
            "Health" => Self::Health,
            "Finance" => Self::Finance,
            "Trade" => Self::Trade,
            "Conflict" => Self::Conflict,
            "Politics" => Self::Politics,
            "Weather" => Self::Weather,
            "Space" => Self::Space,
            "Ocean" => Self::Ocean,
            "Technology" => Self::Technology,
            "Personal" => Self::Personal,
            "Infrastructure" => Self::Infrastructure,
            other => Self::Custom(other.to_owned()),
        }
    }
}

impl fmt::Display for StateDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Custom(name) => write!(f, "{name}"),
            other => fmt::Debug::fmt(other, f),
        }
    }
}

/// Direction of activity change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trend {
    Rising,
    Falling,
    Stable,
    Spike,
    Crash,
}

/// A single domain's state line — tracks activity level and trend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateLine {
    /// The domain this state line tracks.
    pub domain: StateDomain,
    /// Normalised activity level (0.0–1.0).
    pub activity: f32,
    /// Current trend direction.
    pub trend: Trend,
    /// Spatial hotspots within this domain.
    pub hotspots: Vec<GeoHotspot>,
    /// Tick of last update.
    pub last_updated: WorldTick,
    /// Recent activity history for trend detection (not serialised).
    #[serde(skip)]
    pub activity_history: Vec<f32>,
}

impl StateLine {
    /// Create a new state line at zero activity.
    pub fn new(domain: StateDomain) -> Self {
        Self {
            domain,
            activity: 0.0,
            trend: Trend::Stable,
            hotspots: Vec::new(),
            last_updated: WorldTick::zero(),
            activity_history: Vec::new(),
        }
    }

    /// Apply an exponential moving average (EMA) update with alpha = 0.3,
    /// then detect the current trend.
    pub fn update_activity(&mut self, sample: f32, tick: WorldTick) {
        const ALPHA: f32 = 0.3;
        self.activity = ALPHA * sample + (1.0 - ALPHA) * self.activity;
        self.activity_history.push(self.activity);
        if self.activity_history.len() > 60 {
            self.activity_history.remove(0);
        }
        self.trend = self.detect_trend();
        self.last_updated = tick;
    }

    /// Compute trend from the last 3 activity samples.
    fn detect_trend(&self) -> Trend {
        let len = self.activity_history.len();
        if len < 2 {
            return Trend::Stable;
        }

        // Average delta over the last (up to 3) intervals.
        let window = len.min(3);
        let start_idx = len - window;
        let delta =
            (self.activity_history[len - 1] - self.activity_history[start_idx]) / window as f32;

        if delta > 0.15 {
            Trend::Spike
        } else if delta < -0.15 {
            Trend::Crash
        } else if delta > 0.03 {
            Trend::Rising
        } else if delta < -0.03 {
            Trend::Falling
        } else {
            Trend::Stable
        }
    }
}

/// The full world state — a clock plus per-domain state lines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldState {
    /// The world clock driving the simulation.
    pub clock: WorldClock,
    /// Per-domain state lines.
    pub state_lines: BTreeMap<StateDomain, StateLine>,
}

impl WorldState {
    /// Create a world state pre-populated with the 12 default domains.
    pub fn new(clock: WorldClock) -> Self {
        let default_domains = [
            StateDomain::Emergency,
            StateDomain::Health,
            StateDomain::Finance,
            StateDomain::Trade,
            StateDomain::Conflict,
            StateDomain::Politics,
            StateDomain::Weather,
            StateDomain::Space,
            StateDomain::Ocean,
            StateDomain::Technology,
            StateDomain::Personal,
            StateDomain::Infrastructure,
        ];

        let state_lines = default_domains
            .into_iter()
            .map(|d| {
                let sl = StateLine::new(d.clone());
                (d, sl)
            })
            .collect();

        Self { clock, state_lines }
    }

    /// Get a mutable reference to a state line, inserting a default if the
    /// domain doesn't exist yet (useful for `Custom` domains).
    pub fn state_line_mut(&mut self, domain: &StateDomain) -> &mut StateLine {
        self.state_lines
            .entry(domain.clone())
            .or_insert_with(|| StateLine::new(domain.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_display() {
        assert_eq!(StateDomain::Finance.to_string(), "Finance");
        assert_eq!(StateDomain::Custom("Crypto".into()).to_string(), "Crypto");
    }

    #[test]
    fn initial_activity_zero() {
        let sl = StateLine::new(StateDomain::Weather);
        assert!((sl.activity - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn ema_smoothing() {
        let mut sl = StateLine::new(StateDomain::Finance);
        sl.update_activity(1.0, WorldTick(1));
        // EMA: 0.3 * 1.0 + 0.7 * 0.0 = 0.3
        assert!((sl.activity - 0.3).abs() < 1e-5, "got {}", sl.activity);
    }

    #[test]
    fn spike_detection() {
        let mut sl = StateLine::new(StateDomain::Finance);
        // Establish a low baseline first (activity converges near 0.1).
        for i in 0..10 {
            sl.update_activity(0.1, WorldTick(i));
        }
        assert!(matches!(sl.trend, Trend::Stable));
        // Suddenly jump to max — the second tick after the jump should
        // register as a spike because the 3-sample window spans the
        // transition.
        sl.update_activity(1.0, WorldTick(10));
        sl.update_activity(1.0, WorldTick(11));
        assert_eq!(sl.trend, Trend::Spike);
    }

    #[test]
    fn twelve_default_domains() {
        let ws = WorldState::new(WorldClock::default());
        assert_eq!(ws.state_lines.len(), 12);
    }

    #[test]
    fn custom_domain_on_access() {
        let mut ws = WorldState::new(WorldClock::default());
        let domain = StateDomain::Custom("Crypto".into());
        let sl = ws.state_line_mut(&domain);
        assert_eq!(sl.domain, domain);
        assert_eq!(ws.state_lines.len(), 13);
    }
}
