//! Per-substrate connection pool primitives per Spec C₂ §7.1.
//!
//! `life-runtime-pool` is the shared crate that lifed and the four
//! `*-proxy` crates use to bracket every substrate dispatch through a
//! [`Pool`] (semaphore + circuit breaker + ArcSwap-able tonic Channel).
//! Sub-phase E pushes pool ownership down into each `*Proxy` so handlers
//! drop their `pools` field; the proxy method bodies bracket internally.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use arc_swap::ArcSwap;
use opentelemetry::KeyValue;
use opentelemetry::global;
use tokio::sync::Semaphore;
use tonic::transport::Channel;

use crate::breaker::{BreakerState, CircuitBreaker};

/// Pool guard returned by [`Pool::acquire`]. Holds the semaphore permit;
/// callers MUST call exactly one of [`PoolGuard::record_success`] or
/// [`PoolGuard::record_failure`] before drop. If neither is called, the
/// outcome is treated as a failure (defensive — every dispatch path
/// must explicitly classify its outcome).
///
/// Sub-phase E: the guard records to the canonical OTel metric series
/// `life.daemon.dispatch.{count,duration_ms}` and updates the
/// `life.daemon.semaphore.inflight{substrate}` /
/// `life.daemon.breaker_state{substrate}` gauges on drop. Metric writes
/// flow through `opentelemetry::global` so they're a no-op when no
/// meter provider is installed (unit tests, dev daemon without OTLP).
pub struct PoolGuard {
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    inflight: Arc<AtomicUsize>,
    breaker: Arc<CircuitBreaker>,
    recorded: bool,
    /// `true` iff the breaker was in HalfOpen when this guard was issued
    /// AND this caller won the HalfOpen single-trial CAS. Used to decide
    /// whether to emit the trial-outcome metric on drop.
    is_half_open_trial: bool,
    /// Substrate label used for the metric attributes.
    substrate: SubstrateKind,
    /// Wall-clock start used to compute `dispatch.duration_ms`.
    started_at: Instant,
}

impl PoolGuard {
    /// Mark this dispatch successful — feeds the breaker's success counter
    /// AND the OTel `dispatch.count{status="ok"}` series + duration histogram.
    pub fn record_success(mut self) {
        self.recorded = true;
        self.breaker.record_success();
        self.emit_metrics("ok");
        // permit + inflight unwind in Drop.
    }

    /// Mark this dispatch failed — feeds the breaker's failure counter and
    /// the OTel `dispatch.count{status="err"}` series + duration histogram.
    pub fn record_failure(mut self) {
        self.recorded = true;
        self.breaker.record_failure();
        self.emit_metrics("err");
    }

    /// Whether this guard was issued under a HalfOpen single-trial CAS.
    /// Surfaced for instrumentation hooks (the trial-result metric).
    pub fn is_half_open_trial(&self) -> bool {
        self.is_half_open_trial
    }

    fn emit_metrics(&self, status: &'static str) {
        let meter = global::meter("life-runtime-pool");
        let attrs = [
            KeyValue::new("substrate", self.substrate.as_str()),
            KeyValue::new("status", status),
        ];
        meter
            .u64_counter("life.daemon.dispatch.count")
            .build()
            .add(1, &attrs);
        let elapsed_ms = self.started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let dur_attrs = [KeyValue::new("substrate", self.substrate.as_str())];
        meter
            .u64_histogram("life.daemon.dispatch.duration_ms")
            .build()
            .record(elapsed_ms, &dur_attrs);
    }
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        // If the caller forgot to record, treat as failure (defensive).
        if !self.recorded {
            self.breaker.record_failure();
            self.emit_metrics("err");
        }
        let new_inflight = self
            .inflight
            .fetch_sub(1, Ordering::SeqCst)
            .saturating_sub(1);
        // Sub-phase E: keep the semaphore_inflight + breaker_state gauges
        // current on every release.
        let meter = global::meter("life-runtime-pool");
        meter
            .i64_gauge("life.daemon.semaphore.inflight")
            .build()
            .record(
                new_inflight as i64,
                &[KeyValue::new("substrate", self.substrate.as_str())],
            );
        meter.i64_gauge("life.daemon.breaker_state").build().record(
            self.breaker.state().as_metric_value(),
            &[KeyValue::new("substrate", self.substrate.as_str())],
        );
        // permit drops automatically.
        drop(self.permit.take());
    }
}

/// One per-substrate connection pool.
#[derive(Clone)]
pub struct Pool {
    pub channel: Channel,
    pub semaphore: Arc<Semaphore>,
    pub breaker: Arc<CircuitBreaker>,
    pub inflight: Arc<AtomicUsize>,
    pub capacity: u32,
    pub substrate: SubstrateKind,
}

/// Substrate identity tag, used by the metric series to label
/// `life.daemon.breaker_state{substrate=...}` and similar.
///
/// `#[non_exhaustive]` — additional substrates (e.g., a future
/// `Chronos` scheduling substrate) may be added without a breaking
/// change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SubstrateKind {
    /// Arcan agent runtime — agent-loop dispatch, message streaming,
    /// approval handling.
    Arcan,
    /// Lago event journal — namespace open/close, event read/subscribe,
    /// blob store, idempotency persistence, saga journaling.
    Lago,
    /// Haima finance — wallet binding, balance, statement, debit, transfer.
    Haima,
    /// Anima identity — account read/write, profile updates, session
    /// registration + revocation.
    Anima,
    /// Soma kernel daemon — privileged microVM lifecycle. lifed only
    /// reaches soma transitively via arcan; the pool is reserved for
    /// the SpawnChild saga (Spec C₇ post-MVS).
    Soma,
}

impl SubstrateKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubstrateKind::Arcan => "arcan",
            SubstrateKind::Lago => "lago",
            SubstrateKind::Haima => "haima",
            SubstrateKind::Anima => "anima",
            SubstrateKind::Soma => "soma",
        }
    }
}

impl Pool {
    /// Build a pool around an existing channel. The channel may be a
    /// real tonic transport or a test loopback. Capacity is bounded so
    /// the semaphore IS the backpressure — Spec C₂ §8.1.
    pub fn new(channel: Channel, capacity: u32, substrate: SubstrateKind) -> Self {
        Self {
            channel,
            semaphore: Arc::new(Semaphore::new(capacity as usize)),
            breaker: Arc::new(CircuitBreaker::new()),
            inflight: Arc::new(AtomicUsize::new(0)),
            capacity,
            substrate,
        }
    }

    /// Acquire a permit. Per Spec C₂ §7.2:
    ///
    /// - Open + within open-duration → return `Status::unavailable("circuit open")`
    ///   immediately (fail-fast).
    /// - Open + open-duration elapsed → state lazily flips to HalfOpen;
    ///   the caller competes for the single-trial CAS slot. The winner
    ///   gets a guard tagged `is_half_open_trial=true`. Losers see
    ///   `unavailable` and short-circuit.
    /// - HalfOpen with trial active → losers short-circuit.
    /// - Closed → permit awaited from the semaphore (saga / handler
    ///   deadline bounds the wait).
    pub async fn acquire(&self) -> Result<PoolGuard, tonic::Status> {
        let state = self.breaker.state();
        let is_half_open_trial = match state {
            BreakerState::Open => {
                return Err(tonic::Status::unavailable(format!(
                    "{} circuit open",
                    self.substrate.as_str()
                )));
            }
            BreakerState::HalfOpen => {
                if !self.breaker.try_acquire_half_open_trial() {
                    return Err(tonic::Status::unavailable(format!(
                        "{} circuit half-open (trial in flight)",
                        self.substrate.as_str()
                    )));
                }
                true
            }
            BreakerState::Closed => false,
        };
        let permit = match self.semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => {
                // I1 fix (PR #1062 code-quality review): if the semaphore is
                // closed AFTER we won the HalfOpen trial CAS, release the
                // trial slot before erroring out so the breaker doesn't
                // deadlock in HalfOpen. Without this, `half_open_trial_active`
                // would stay `true` forever and every future HalfOpen call
                // would short-circuit. In practice this only fires during
                // shutdown but the contract must be sound.
                if is_half_open_trial {
                    self.breaker.release_half_open_trial();
                }
                return Err(tonic::Status::unavailable("semaphore closed"));
            }
        };
        let new_inflight = self
            .inflight
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        // Sub-phase E: bump the semaphore_inflight gauge as soon as the
        // permit lands. Spec C₂ §9.3.
        global::meter("life-runtime-pool")
            .i64_gauge("life.daemon.semaphore.inflight")
            .build()
            .record(
                new_inflight as i64,
                &[KeyValue::new("substrate", self.substrate.as_str())],
            );
        Ok(PoolGuard {
            permit: Some(permit),
            inflight: Arc::clone(&self.inflight),
            breaker: Arc::clone(&self.breaker),
            recorded: false,
            is_half_open_trial,
            substrate: self.substrate,
            started_at: Instant::now(),
        })
    }

    /// Number of in-flight dispatches against this pool — feeds
    /// `life.daemon.semaphore.inflight{substrate=...}`.
    pub fn inflight(&self) -> usize {
        self.inflight.load(Ordering::SeqCst)
    }

    /// Current breaker state — feeds `life.daemon.breaker_state{...}`.
    pub fn breaker_state(&self) -> BreakerState {
        self.breaker.state()
    }

    pub fn substrate(&self) -> SubstrateKind {
        self.substrate
    }
}

/// Holder for all five substrate pools. `ArcSwap<Pool>` allows hot-swap
/// without disrupting in-flight dispatches.
pub struct SubstratePools {
    pub arcan: Arc<ArcSwap<Pool>>,
    pub lago: Arc<ArcSwap<Pool>>,
    pub haima: Arc<ArcSwap<Pool>>,
    pub anima: Arc<ArcSwap<Pool>>,
    pub soma: Arc<ArcSwap<Pool>>,
}

impl SubstratePools {
    pub fn new(initial: SubstratePoolsInitial) -> Self {
        Self {
            arcan: Arc::new(ArcSwap::from_pointee(initial.arcan)),
            lago: Arc::new(ArcSwap::from_pointee(initial.lago)),
            haima: Arc::new(ArcSwap::from_pointee(initial.haima)),
            anima: Arc::new(ArcSwap::from_pointee(initial.anima)),
            soma: Arc::new(ArcSwap::from_pointee(initial.soma)),
        }
    }

    /// Hot-swap the named substrate's pool. Used when the substrate
    /// restarts and lifed re-dials its socket.
    pub fn swap_arcan(&self, new: Pool) {
        self.arcan.store(Arc::new(new));
    }
    pub fn swap_lago(&self, new: Pool) {
        self.lago.store(Arc::new(new));
    }
    pub fn swap_haima(&self, new: Pool) {
        self.haima.store(Arc::new(new));
    }
    pub fn swap_anima(&self, new: Pool) {
        self.anima.store(Arc::new(new));
    }
    pub fn swap_soma(&self, new: Pool) {
        self.soma.store(Arc::new(new));
    }
}

/// Initial-value bundle used by [`SubstratePools::new`]. Each substrate's
/// pool is constructed once at boot from its UDS channel + spec-driven
/// capacity (`arcan: 32`, `lago: 64`, `haima: 16`, `anima: 16`, `soma: 8`)
/// and handed to [`SubstratePools::new`].
pub struct SubstratePoolsInitial {
    /// Arcan substrate pool — capacity defaults to 32 per Spec C₂ §7.1.
    pub arcan: Pool,
    /// Lago substrate pool — capacity defaults to 64 per Spec C₂ §7.1.
    pub lago: Pool,
    /// Haima substrate pool — capacity defaults to 16 per Spec C₂ §7.1.
    pub haima: Pool,
    /// Anima substrate pool — capacity defaults to 16 per Spec C₂ §7.1.
    pub anima: Pool,
    /// Soma substrate pool — capacity defaults to 8 per Spec C₂ §7.1.
    /// Reachable only from Spec C₇ SpawnChild saga (admin-plane).
    pub soma: Pool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a Pool against a dummy channel for testing. Spec C₂ §8.1
    /// caveat: a tonic Channel created with a never-connecting endpoint
    /// is fine for guard-only tests because we never call through it.
    fn dummy_channel() -> Channel {
        // Endpoint::try_from is infallible for a static valid URL.
        tonic::transport::Endpoint::try_from("http://[::]:0")
            .expect("endpoint")
            .connect_lazy()
    }

    #[tokio::test]
    async fn acquire_returns_guard_under_capacity() {
        let pool = Pool::new(dummy_channel(), 4, SubstrateKind::Lago);
        assert_eq!(pool.inflight(), 0);
        let guard = pool.acquire().await.expect("acquire");
        assert_eq!(pool.inflight(), 1);
        guard.record_success();
        assert_eq!(pool.inflight(), 0);
    }

    #[tokio::test]
    async fn breaker_open_fails_acquire_fast() {
        let pool = Pool::new(dummy_channel(), 4, SubstrateKind::Arcan);
        for _ in 0..crate::breaker::FAILURE_THRESHOLD {
            pool.breaker.record_failure();
        }
        assert_eq!(pool.breaker_state(), BreakerState::Open);
        let result = pool.acquire().await;
        let err = match result {
            Ok(_) => panic!("acquire must fail when breaker open"),
            Err(e) => e,
        };
        assert_eq!(err.code(), tonic::Code::Unavailable);
        assert!(err.message().contains("arcan"));
    }

    #[tokio::test]
    async fn record_failure_feeds_breaker() {
        let pool = Pool::new(dummy_channel(), 4, SubstrateKind::Haima);
        for _ in 0..crate::breaker::FAILURE_THRESHOLD {
            let g = pool.acquire().await.expect("acquire");
            g.record_failure();
        }
        assert_eq!(pool.breaker_state(), BreakerState::Open);
    }

    #[tokio::test]
    async fn capacity_caps_concurrent_inflight() {
        let pool = Pool::new(dummy_channel(), 2, SubstrateKind::Anima);
        let g1 = pool.acquire().await.expect("acquire 1");
        let g2 = pool.acquire().await.expect("acquire 2");
        assert_eq!(pool.inflight(), 2);
        // Third acquire must block until one of the first two is dropped.
        let third = tokio::spawn({
            let pool = pool.clone();
            async move { pool.acquire().await }
        });
        // Yield + give it a beat — should still be pending.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!third.is_finished(), "third acquire blocked on capacity");
        g1.record_success();
        // Now the third acquire should resolve.
        let g3 = third.await.expect("join").expect("acquire 3");
        g2.record_success();
        g3.record_success();
    }

    #[tokio::test]
    async fn substrate_pools_holds_five_arcswaps() {
        let initial = SubstratePoolsInitial {
            arcan: Pool::new(dummy_channel(), 32, SubstrateKind::Arcan),
            lago: Pool::new(dummy_channel(), 64, SubstrateKind::Lago),
            haima: Pool::new(dummy_channel(), 16, SubstrateKind::Haima),
            anima: Pool::new(dummy_channel(), 16, SubstrateKind::Anima),
            soma: Pool::new(dummy_channel(), 8, SubstrateKind::Soma),
        };
        let pools = SubstratePools::new(initial);
        assert_eq!(pools.arcan.load().capacity, 32);
        assert_eq!(pools.lago.load().capacity, 64);
        assert_eq!(pools.haima.load().capacity, 16);
        assert_eq!(pools.anima.load().capacity, 16);
        assert_eq!(pools.soma.load().capacity, 8);
    }

    #[tokio::test]
    async fn drop_without_record_marks_failure() {
        let pool = Pool::new(dummy_channel(), 4, SubstrateKind::Soma);
        {
            let _g = pool.acquire().await.expect("acquire");
            // dropped without record — should count as failure.
        }
        // Single failure isn't enough to trip Open by consecutive count
        // alone; test that consecutive_failures bumped via state transition.
        // We assert by making 4 more failures and seeing the breaker open.
        for _ in 0..4 {
            let g = pool.acquire().await.expect("acquire");
            g.record_failure();
        }
        assert_eq!(pool.breaker_state(), BreakerState::Open);
    }

    /// Sub-phase E: under HalfOpen stampede the pool admits exactly one
    /// trial guard; concurrent acquires receive `Status::unavailable`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn half_open_stampede_admits_one_guard() {
        use std::sync::atomic::AtomicUsize;
        let pool = Arc::new(Pool::new(dummy_channel(), 100, SubstrateKind::Lago));
        // Trip Open then advance the open_until past so the lazy
        // transition flips to HalfOpen.
        for _ in 0..crate::breaker::FAILURE_THRESHOLD {
            pool.breaker.record_failure();
        }
        pool.breaker.force_open_window_elapsed();
        assert_eq!(pool.breaker_state(), BreakerState::HalfOpen);

        let admitted = Arc::new(AtomicUsize::new(0));
        let rejected = Arc::new(AtomicUsize::new(0));
        let mut joins = Vec::with_capacity(100);
        for _ in 0..100 {
            let pool = Arc::clone(&pool);
            let admitted = Arc::clone(&admitted);
            let rejected = Arc::clone(&rejected);
            joins.push(tokio::spawn(async move {
                match pool.acquire().await {
                    Ok(g) => {
                        admitted.fetch_add(1, Ordering::SeqCst);
                        // Hold the guard so the stampede sees the slot
                        // taken; do NOT record an outcome until all peers
                        // have raced.
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        g.record_success();
                    }
                    Err(_) => {
                        rejected.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }
        for j in joins {
            let _ = j.await;
        }
        assert_eq!(admitted.load(Ordering::SeqCst), 1);
        assert_eq!(rejected.load(Ordering::SeqCst), 99);
    }
}
