//! Per-substrate connection pools per Spec C₂ §7.1.
//!
//! Each substrate (`arcan`, `lago`, `haima`, `anima`, `soma`) gets a
//! dedicated [`Pool`] holding the tonic [`Channel`], a bounded
//! [`Semaphore`] (capacity per Spec C₂ §7.1 — arcan: 32, lago: 64,
//! haima: 16, anima: 16, soma: 8), and an `Arc<CircuitBreaker>`. Every
//! substrate call brackets with [`Pool::acquire`] returning a
//! [`PoolGuard`]; the guard records success/failure on drop or via the
//! explicit `record_*` methods.
//!
//! Hot-swap: the [`SubstratePools`] holder uses `ArcSwap<Pool>` per
//! substrate so a hot-config-reload (D-stretch / sub-phase E) can
//! swap the underlying tonic channel without taking out lifed.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arc_swap::ArcSwap;
use tokio::sync::Semaphore;
use tonic::transport::Channel;

use crate::routing::breaker::{BreakerState, CircuitBreaker};

/// Pool guard returned by [`Pool::acquire`]. Holds the semaphore permit;
/// callers MUST call exactly one of [`PoolGuard::record_success`] or
/// [`PoolGuard::record_failure`] before drop. If neither is called, the
/// outcome is treated as a failure (defensive — every dispatch path
/// must explicitly classify its outcome).
pub struct PoolGuard {
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    inflight: Arc<AtomicUsize>,
    breaker: Arc<CircuitBreaker>,
    recorded: bool,
}

impl PoolGuard {
    /// Mark this dispatch successful — feeds the breaker's success counter.
    pub fn record_success(mut self) {
        self.recorded = true;
        self.breaker.record_success();
        // permit + inflight unwind in Drop.
    }

    /// Mark this dispatch failed — feeds the breaker's failure counter.
    pub fn record_failure(mut self) {
        self.recorded = true;
        self.breaker.record_failure();
    }
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        // If the caller forgot to record, treat as failure (defensive).
        if !self.recorded {
            self.breaker.record_failure();
        }
        self.inflight.fetch_sub(1, Ordering::SeqCst);
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubstrateKind {
    Arcan,
    Lago,
    Haima,
    Anima,
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

    /// Acquire a permit. If the breaker is Open the call returns
    /// `Status::unavailable("circuit open")` immediately — fail-fast.
    /// If the semaphore is exhausted the call awaits a permit; the
    /// caller's deadline (saga / handler-level) bounds the wait.
    pub async fn acquire(&self) -> Result<PoolGuard, tonic::Status> {
        if self.breaker.state() == BreakerState::Open {
            return Err(tonic::Status::unavailable(format!(
                "{} circuit open",
                self.substrate.as_str()
            )));
        }
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| tonic::Status::unavailable("semaphore closed"))?;
        self.inflight.fetch_add(1, Ordering::SeqCst);
        Ok(PoolGuard {
            permit: Some(permit),
            inflight: Arc::clone(&self.inflight),
            breaker: Arc::clone(&self.breaker),
            recorded: false,
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

pub struct SubstratePoolsInitial {
    pub arcan: Pool,
    pub lago: Pool,
    pub haima: Pool,
    pub anima: Pool,
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
        for _ in 0..super::super::breaker::FAILURE_THRESHOLD {
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
        for _ in 0..super::super::breaker::FAILURE_THRESHOLD {
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
}
