use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Monotonically increasing tick counter for the world clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorldTick(pub u64);

impl WorldTick {
    /// The zero tick (epoch start).
    pub fn zero() -> Self {
        Self(0)
    }

    /// Return the next tick value.
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl fmt::Display for WorldTick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tick:{}", self.0)
    }
}

/// World clock — drives the simulation cadence.
///
/// Each call to [`advance`](WorldClock::advance) increments the tick counter and
/// updates wall-time.  The `hz` field controls the nominal tick rate, while
/// `time_scale` allows simulated time to run faster or slower than real time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldClock {
    /// Current tick number.
    pub tick: WorldTick,
    /// UTC timestamp of tick zero.
    pub epoch: DateTime<Utc>,
    /// Wall-clock time of the most recent tick.
    pub wall_time: DateTime<Utc>,
    /// Nominal ticks per second.
    pub hz: f64,
    /// Multiplier for simulated time (1.0 = real-time).
    pub time_scale: f64,
}

impl WorldClock {
    /// Create a new clock at tick zero running at the given frequency.
    pub fn new(hz: f64) -> Self {
        let now = Utc::now();
        Self {
            tick: WorldTick::zero(),
            epoch: now,
            wall_time: now,
            hz,
            time_scale: 1.0,
        }
    }

    /// Advance the clock by one tick, updating wall-time to now.
    pub fn advance(&mut self) {
        self.tick = self.tick.next();
        self.wall_time = Utc::now();
    }

    /// Real seconds elapsed since epoch.
    pub fn elapsed_seconds(&self) -> f64 {
        let dur = self.wall_time.signed_duration_since(self.epoch);
        dur.num_milliseconds() as f64 / 1000.0
    }

    /// Simulated seconds = `ticks / hz * time_scale`.
    pub fn simulated_seconds(&self) -> f64 {
        (self.tick.0 as f64 / self.hz) * self.time_scale
    }
}

impl Default for WorldClock {
    fn default() -> Self {
        Self::new(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_zero() {
        let clock = WorldClock::default();
        assert_eq!(clock.tick, WorldTick::zero());
    }

    #[test]
    fn advance_increments() {
        let mut clock = WorldClock::default();
        clock.advance();
        assert_eq!(clock.tick, WorldTick(1));
        clock.advance();
        assert_eq!(clock.tick, WorldTick(2));
    }

    #[test]
    fn simulated_seconds_at_1hz() {
        let mut clock = WorldClock::new(1.0);
        clock.tick = WorldTick(10);
        assert!((clock.simulated_seconds() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn world_tick_ordering() {
        assert!(WorldTick(0) < WorldTick(1));
        assert!(WorldTick(100) > WorldTick(42));
        assert_eq!(WorldTick(7), WorldTick(7));
    }
}
