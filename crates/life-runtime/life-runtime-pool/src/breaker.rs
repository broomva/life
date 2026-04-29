//! Hand-rolled circuit breaker per Spec C₂ §7.2.
//!
//! State machine:
//!
//! ```text
//! Closed ──5 consecutive failures or err-rate > 50% / 30s──▶ Open
//!   ▲                                                          │
//!   │                                              after 10s   │
//!   │           any successful trial → Closed                  ▼
//!   └────────────────────────────────────────────────────── HalfOpen
//!                                                              │
//!                       any failed trial → Open                 │
//!                       ◀──────────────────────────────────────┘
//! ```
//!
//! All state transitions are wait-free atomic operations. Failure-rate
//! tracking uses a 30-second sliding window of `(success_count,
//! failure_count)` packed into one `AtomicU64` so observers don't tear.
//!
//! ## HalfOpen single-trial CAS — Sub-phase E
//!
//! Spec C₂ §7.2 calls for a **single trial request** in HalfOpen. The
//! field [`CircuitBreaker::half_open_trial_active`] is a wait-free
//! `AtomicBool` whose CAS-acquire gates entry into HalfOpen. Once a
//! caller wins the CAS its single trial proceeds; concurrent callers
//! that lose the CAS see the breaker as if it were still Open and
//! short-circuit. On the trial outcome the CAS slot is reset to false:
//! a successful trial closes the breaker and frees the slot for the
//! Closed state's normal flow; a failed trial re-opens the breaker.
//!
//! Tested under stampede in the unit tests below: 100 concurrent calls
//! into HalfOpen result in exactly 1 trial proceeding.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering};
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
///
/// `#[non_exhaustive]` — additional states (e.g., `Quarantined` for
/// long-term outages) may be added without a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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
    /// Sub-phase E: HalfOpen single-trial CAS gate. Set to `true` by the
    /// first caller to enter HalfOpen; the trial outcome resets it to
    /// `false`. Spec C₂ §7.2.
    half_open_trial_active: AtomicBool,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(STATE_CLOSED),
            consecutive_failures: AtomicU32::new(0),
            open_until_unix_nanos: AtomicI64::new(0),
            window_counters: AtomicU64::new(0),
            window_start_unix_nanos: AtomicI64::new(unix_nanos_now()),
            half_open_trial_active: AtomicBool::new(false),
        }
    }

    /// Read the current state. If the breaker was Open and the
    /// open-duration has elapsed, transition to HalfOpen lazily.
    ///
    /// IMPORTANT: this method is read-only — it does NOT reserve a
    /// HalfOpen trial slot. Callers that intend to dispatch a HalfOpen
    /// trial must call [`Self::try_acquire_half_open_trial`] to win the
    /// single-trial CAS gate. Concurrent observers that read HalfOpen
    /// without winning the CAS must short-circuit as if the breaker is
    /// still Open.
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

    /// Sub-phase E: single-trial CAS gate for HalfOpen.
    ///
    /// Returns `true` iff this caller acquires the lone trial slot. All
    /// concurrent callers see `false` and must treat the breaker as Open.
    /// The slot resets on the next [`Self::record_success`] (which closes
    /// the breaker) or [`Self::record_failure`] (which re-opens it).
    ///
    /// Spec C₂ §7.2 invariant: at most one in-flight trial in HalfOpen.
    pub fn try_acquire_half_open_trial(&self) -> bool {
        self.half_open_trial_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Whether a HalfOpen trial is currently in flight. Used by
    /// observability + tests.
    pub fn half_open_trial_in_flight(&self) -> bool {
        self.half_open_trial_active.load(Ordering::SeqCst)
    }

    /// Release the HalfOpen trial slot WITHOUT recording a result.
    ///
    /// Used when a caller has won the trial CAS via
    /// [`Self::try_acquire_half_open_trial`] but cannot proceed (e.g.
    /// `Pool::acquire`'s semaphore close on shutdown). Without this
    /// release, `half_open_trial_active` would remain `true` forever
    /// and every future HalfOpen attempt would short-circuit — the
    /// breaker would deadlock in HalfOpen.
    ///
    /// Per Spec C₂ §7.2, this preserves the "single trial" invariant:
    /// no trial actually ran, so the slot is freed for the next caller.
    /// Callers should NOT call `record_success` / `record_failure`
    /// after this; that contract is enforced by the `PoolGuard` Drop
    /// impl, which only feeds the breaker if a permit was acquired.
    pub fn release_half_open_trial(&self) {
        self.half_open_trial_active.store(false, Ordering::SeqCst);
    }

    /// Record a successful call. Resets consecutive-failures, slides
    /// the window forward, and transitions HalfOpen→Closed (releasing
    /// the trial slot).
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.bump_window(true);
        let s = self.state.load(Ordering::SeqCst);
        if s == STATE_HALF_OPEN {
            // One successful trial closes the breaker.
            self.state.store(STATE_CLOSED, Ordering::SeqCst);
            // Release the trial slot so future Closed-state work flows
            // cleanly; subsequent Open→HalfOpen transitions will
            // re-acquire via `try_acquire_half_open_trial`.
            self.half_open_trial_active.store(false, Ordering::SeqCst);
        }
    }

    /// Record a failed call. Trips Open if either the consecutive-failure
    /// threshold or the windowed failure-rate exceeds the limit. A failed
    /// trial in HalfOpen re-opens the breaker and releases the trial slot.
    pub fn record_failure(&self) {
        let n = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        self.bump_window(false);
        // A single failed trial in HalfOpen re-opens the breaker — handle
        // this BEFORE the threshold check so we always release the trial
        // slot.
        let s = self.state.load(Ordering::SeqCst);
        if s == STATE_HALF_OPEN {
            self.trip_open();
            self.half_open_trial_active.store(false, Ordering::SeqCst);
            return;
        }
        if n >= FAILURE_THRESHOLD {
            self.trip_open();
            return;
        }
        if self.windowed_rate_exceeded() {
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

#[cfg(any(test, feature = "test-support"))]
impl CircuitBreaker {
    /// Test-only: force the open_until anchor into the past so the lazy
    /// HalfOpen transition fires on the next state read. Behind a
    /// `test-support` feature so production code can't reach it.
    pub fn force_open_window_elapsed(&self) {
        self.open_until_unix_nanos
            .store(unix_nanos_now() - 1, Ordering::SeqCst);
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
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

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

    /// Sub-phase E: 100 concurrent attempts to acquire the HalfOpen
    /// single-trial slot must produce exactly one winner. Spec C₂ §7.2.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn half_open_single_trial_under_stampede() {
        let cb = Arc::new(CircuitBreaker::new());
        // Drive into HalfOpen by tripping then waiting out the open window.
        for _ in 0..FAILURE_THRESHOLD {
            cb.record_failure();
        }
        // Force the open_until into the past so the next state() flips
        // Open→HalfOpen via the lazy transition.
        cb.force_open_window_elapsed();
        assert_eq!(cb.state(), BreakerState::HalfOpen);

        let winners = Arc::new(AtomicUsize::new(0));
        let mut joins = Vec::with_capacity(100);
        for _ in 0..100 {
            let cb = Arc::clone(&cb);
            let winners = Arc::clone(&winners);
            joins.push(tokio::spawn(async move {
                if cb.try_acquire_half_open_trial() {
                    winners.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for j in joins {
            let _ = j.await;
        }
        assert_eq!(
            winners.load(Ordering::SeqCst),
            1,
            "exactly one trial wins under stampede"
        );
    }

    #[test]
    fn half_open_trial_slot_releases_on_success() {
        let cb = CircuitBreaker::new();
        // Trip Open then move to HalfOpen.
        for _ in 0..FAILURE_THRESHOLD {
            cb.record_failure();
        }
        cb.force_open_window_elapsed();
        let _ = cb.state(); // promote to HalfOpen
        assert!(cb.try_acquire_half_open_trial(), "first acquire wins");
        assert!(
            !cb.try_acquire_half_open_trial(),
            "second concurrent acquire loses"
        );
        // Successful trial closes the breaker AND releases the slot.
        cb.record_success();
        assert_eq!(cb.state(), BreakerState::Closed);
        assert!(!cb.half_open_trial_in_flight());
    }

    #[test]
    fn half_open_trial_slot_releases_on_failure() {
        let cb = CircuitBreaker::new();
        for _ in 0..FAILURE_THRESHOLD {
            cb.record_failure();
        }
        cb.force_open_window_elapsed();
        let _ = cb.state(); // promote to HalfOpen
        assert!(cb.try_acquire_half_open_trial());
        // Failed trial re-opens AND releases the slot for the next
        // open-duration cycle.
        cb.record_failure();
        assert_eq!(cb.state(), BreakerState::Open);
        assert!(!cb.half_open_trial_in_flight());
    }

    /// I1 fix (PR #1062 code-quality review): explicit `release_half_open_trial`
    /// frees the CAS slot WITHOUT recording success or failure. Used by
    /// `Pool::acquire` when the semaphore acquire fails AFTER winning
    /// the HalfOpen trial CAS — without the release the breaker would
    /// deadlock in HalfOpen.
    #[test]
    fn release_half_open_trial_frees_slot_without_recording() {
        let cb = CircuitBreaker::new();
        for _ in 0..FAILURE_THRESHOLD {
            cb.record_failure();
        }
        cb.force_open_window_elapsed();
        let _ = cb.state(); // promote to HalfOpen
        assert!(cb.try_acquire_half_open_trial(), "first acquire wins");
        assert!(cb.half_open_trial_in_flight());
        // Release WITHOUT recording — simulates the semaphore-closed
        // path in `Pool::acquire`.
        cb.release_half_open_trial();
        assert!(
            !cb.half_open_trial_in_flight(),
            "slot released; subsequent callers can re-acquire"
        );
        // Slot is freed: a new caller can acquire the trial.
        assert!(
            cb.try_acquire_half_open_trial(),
            "next caller acquires the freed slot"
        );
        // Breaker stays HalfOpen because no result was recorded.
        assert_eq!(cb.state(), BreakerState::HalfOpen);
    }
}
