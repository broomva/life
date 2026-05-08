//! KV cache trait — the L0..L3 memory hierarchy contract.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::InferenceError;
use crate::ids::KvKey;

/// Opaque handle to a cached KV slice. Cheap to copy; stable for the
/// lifetime of the cache or until [`KvCache::evict`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KvHandle(pub u64);

/// RAII guard returned by [`KvCache::pin`]. While held, the underlying
/// slice is guaranteed to stay in device memory (i.e., not evicted to
/// L1/L2/L3). Dropping the guard releases the pin.
pub struct KvPinGuard {
    /// Closure invoked on drop to release the pin in the cache.
    pub(crate) on_drop: Box<dyn FnOnce() + Send + Sync>,
    /// The handle this guard pins.
    pub(crate) handle: KvHandle,
}

impl KvPinGuard {
    /// The handle this guard pins.
    #[must_use]
    pub fn handle(&self) -> KvHandle {
        self.handle
    }
}

impl Drop for KvPinGuard {
    fn drop(&mut self) {
        // Replace the FnOnce with a no-op so we can call it.
        let f = std::mem::replace(&mut self.on_drop, Box::new(|| {}));
        f();
    }
}

/// Opaque AnimaId reference used as the scoping key for [`KvCache::persist`]
/// and [`KvCache::rehydrate`]. Backends do not validate this — Anima
/// (`crates/anima/anima-identity`) is the source of truth; this is
/// passed-through.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AnimaIdRef(pub Arc<str>);

impl AnimaIdRef {
    /// Construct from any string-convertible input.
    #[must_use]
    pub fn new(did: impl Into<String>) -> Self {
        Self(Arc::from(did.into()))
    }

    /// Borrow as `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lago object identifier returned by [`KvCache::persist`]. Lifetime
/// is governed by Lago retention policy. AnimaId-scoped per L5-D6.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LagoOidRef(pub Arc<str>);

/// The KV cache contract. Backends provide their own impl; the dev /
/// test impl is [`crate::InMemoryKvCache`].
///
/// Locked decisions: L5-D2 (Lago-backed by default), L5-D6 (AnimaId-
/// scoped), L5-D5 (no tool runtime — KV is for model state only).
pub trait KvCache: Send + Sync + 'static {
    /// Look up a cached slice. `None` on miss.
    fn lookup(&self, key: &KvKey) -> Option<KvHandle>;

    /// Copy-on-write fork. The returned handle observes `base` until
    /// the first divergent write, then diverges privately. Cheap.
    fn fork(&self, base: KvHandle) -> KvHandle;

    /// Drop a cached slice. Pinned handles are not evicted; the call
    /// is a no-op until all [`KvPinGuard`]s for `handle` are dropped.
    fn evict(&self, handle: KvHandle);

    /// Persist a slice into Lago, scoped by `anima`. The returned
    /// `LagoOidRef` is durable across sessions and re-resolvable
    /// via [`KvCache::rehydrate`].
    fn persist<'a>(
        &'a self,
        handle: KvHandle,
        anima: &'a AnimaIdRef,
    ) -> Pin<Box<dyn Future<Output = Result<LagoOidRef, InferenceError>> + Send + 'a>>;

    /// Rehydrate a Lago-stored slice back into a [`KvHandle`]. Returns
    /// [`InferenceError::Backend`] with [`crate::CloseCode::AnimaInvalidated`]
    /// if `anima` doesn't match the OID's recorded scope.
    fn rehydrate<'a>(
        &'a self,
        oid: &'a LagoOidRef,
        anima: &'a AnimaIdRef,
    ) -> Pin<Box<dyn Future<Output = Result<KvHandle, InferenceError>> + Send + 'a>>;

    /// Pin `handle` in device memory for the lifetime of the returned
    /// guard. Use sparingly — pinned slices block eviction and can
    /// stall the L1 → L2 spill.
    fn pin(&self, handle: KvHandle) -> KvPinGuard;
}
