//! Hand-rolled circuit breaker per Spec C₂ §7.2.
//!
//! State machine:
//!
//! ```text
//! Closed ──5 consecutive failures or err-rate > 50% / 30s──▶ Open
//!   ▲                                                          │
//!   │                                              after 10s   │
//!   │             one successful trial                         ▼
//!   └────────────────────────────────────────────────────── HalfOpen
//!                                                              │
//!                       failed trial                            │
//!                       ◀──────────────────────────────────────┘
//! ```
//!
//! All state transitions are wait-free atomic operations. Failure-rate
//! tracking uses a 30-second sliding window of `(success_count,
//! failure_count)` packed into one `AtomicU64` so observers don't tear.
//!
//! When the optional `failsafe-breaker` feature is enabled, the lifed
//! pool layer can swap in `failsafe-rs` instead — see
//! `routing/pools.rs::Pool::new_with_breaker`. The hand-rolled
//! implementation is the M5 default.

use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// Default Open duration before transitioning to HalfOpen — Spec C₂ §7.2.
pub const OPEN_DURATION: Duration = Duration::from_secs(10);
/// Default consecutive-failure threshold — Spec C₂ §7.2.
pub const FAILURE_THRESHOLD: u32 = 5;
/// Default sliding-window length for the failure-rate tripwire — Spec C₂ §7.2.
pub const RATE_WINDOW: Duration = Duration::from_secs(30);
/// Default failure rate that trips the breaker (over `RATE_WINDOW`).
/// Spec C₂ §7.2 specifies 50 %.
pub const RATE_THRESHOLD: f32 = 0.5;
/// Minimum sample size before the rate-tripwire engages. Below this we
/// rely on `FAILURE_THRESHOLD` alone.
pub const RATE_MIN_SAMPLES: u32 = 8;

const STATE_CLOSED: u8 = 0;
const STATE_HALF_OPEN: u8 = 1;
const STATE_OPEN: u8 = 2;

/// Public state surface — the gauge series `life.daemon.breaker_state`
/// emits 0/1/2 for Closed/HalfOpen/Open respectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    HalfOpen,
    Open,
}

impl BreakerState {
    /// Numeric encoding for the metric series — Spec C₂ §9.3.
    pub fn as_metric_value(&self) -> i64 {
        match self {
            BreakerState::Closed => 0,
            BreakerState::HalfOpen => 1,
            BreakerState::Open => 2,
        }
    }
}

/// Hand-rolled circuit breaker. Cheap to construct — every field is an
/// atomic. Designed to be wrapped in `Arc` and shared across a pool.
pub struct CircuitBreaker {
    state: AtomicU8,
    consecutive_failures: AtomicU32,
    open_until_unix_nanos: AtomicI64,
    /// Sliding-window counters packed as `(success << 32) | failures` and
    /// the window-start unix-nanos. When the window age exceeds
    /// `RATE_WINDOW` the counters reset.
    window_counters: AtomicU64,
    window_start_unix_nanos: AtomicI64,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(STATE_CLOSED),
            consecutive_failures: AtomicU32::new(0),
            open_until_unix_nanos: AtomicI64::new(0),
            window_counters: AtomicU64::new(0),
            window_start_unix_nanos: AtomicI64::new(unix_nanos_now()),
        }
    }

    /// Read the current state. If the breaker was Open and the
    /// open-duration has elapsed, transition to HalfOpen lazily.
    pub fn state(&self) -> BreakerState {
        let now = unix_nanos_now();
        let s = self.state.load(Ordering::SeqCst);
        if s == STATE_OPEN {
            let until = self.open_until_unix_nanos.load(Ordering::SeqCst);
            if now >= until {
                let _ = self.state.compare_exchange(
                    STATE_OPEN,
                    STATE_HALF_OPEN,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                return BreakerState::HalfOpen;
            }
            return BreakerState::Open;
        }
        match s {
            STATE_CLOSED => BreakerState::Closed,
            STATE_HALF_OPEN => BreakerState::HalfOpen,
            _ => BreakerState::Open,
        }
    }

    /// Record a successful call. Resets consecutive-failures, slides
    /// the window forward, and transitions HalfOpen→Closed.
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.bump_window(true);
        let s = self.state.load(Ordering::SeqCst);
        if s == STATE_HALF_OPEN {
            // One successful trial closes the breaker.
            self.state.store(STATE_CLOSED, Ordering::SeqCst);
        }
    }

    /// Record a failed call. Trips Open if either the consecutive-failure
    /// threshold or the windowed failure-rate exceeds the limit.
    pub fn record_failure(&self) {
        let n = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        self.bump_window(false);
        if n >= FAILURE_THRESHOLD {
            self.trip_open();
            return;
        }
        if self.windowed_rate_exceeded() {
            self.trip_open();
        }
        // A single failed trial in HalfOpen re-opens the breaker.
        let s = self.state.load(Ordering::SeqCst);
        if s == STATE_HALF_OPEN {
            self.trip_open();
        }
    }

    fn trip_open(&self) {
        self.state.store(STATE_OPEN, Ordering::SeqCst);
        self.open_until_unix_nanos.store(
            unix_nanos_now() + OPEN_DURATION.as_nanos() as i64,
            Ordering::SeqCst,
        );
    }

    fn bump_window(&self, success: bool) {
        let now = unix_nanos_now();
        let started = self.window_start_unix_nanos.load(Ordering::SeqCst);
        if now - started > RATE_WINDOW.as_nanos() as i64 {
            // Reset the window. Race-tolerant: if another thread already
            // reset it, our store overwrites with the same value.
            self.window_counters.store(0, Ordering::SeqCst);
            self.window_start_unix_nanos.store(now, Ordering::SeqCst);
        }
        let delta: u64 = if success { 1u64 << 32 } else { 1 };
        self.window_counters.fetch_add(delta, Ordering::SeqCst);
    }

    fn windowed_rate_exceeded(&self) -> bool {
        let packed = self.window_counters.load(Ordering::SeqCst);
        let success = (packed >> 32) as u32;
        let failures = (packed & 0xffff_ffff) as u32;
        let total = success.saturating_add(failures);
        if total < RATE_MIN_SAMPLES {
            return false;
        }
        let rate = failures as f32 / total as f32;
        rate >= RATE_THRESHOLD
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

fn unix_nanos_now() -> i64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_initial_state() {
        let cb = CircuitBreaker::new();
        assert_eq!(cb.state(), BreakerState::Closed);
    }

    #[test]
    fn opens_after_threshold_failures() {
        let cb = CircuitBreaker::new();
        for _ in 0..FAILURE_THRESHOLD {
            cb.record_failure();
        }
        assert_eq!(cb.state(), BreakerState::Open);
    }

    #[test]
    fn closes_on_success_after_failures() {
        let cb = CircuitBreaker::new();
        for _ in 0..2 {
            cb.record_failure();
        }
        cb.record_success();
        assert_eq!(cb.state(), BreakerState::Closed);
    }

    #[test]
    fn rate_tripwire_with_minimum_samples() {
        let cb = CircuitBreaker::new();
        // Push enough samples to trip rate but stay below the consecutive
        // threshold by interleaving a success.
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        cb.record_failure();
        cb.record_failure();
        // 6 fail / 8 total = 0.75 rate ⇒ trips Open.
        assert_eq!(cb.state(), BreakerState::Open);
    }

    #[test]
    fn metric_encoding_is_canonical() {
        assert_eq!(BreakerState::Closed.as_metric_value(), 0);
        assert_eq!(BreakerState::HalfOpen.as_metric_value(), 1);
        assert_eq!(BreakerState::Open.as_metric_value(), 2);
    }
}
