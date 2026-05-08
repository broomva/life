//! In-memory [`KvCache`] for unit tests and dev-mode runtimes.
//! Persistence is a no-op (returns a fake OID); fork is reference-
//! counted; pin tracking is exact.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::InferenceError;
use crate::ids::KvKey;
use crate::kv::{AnimaIdRef, KvCache, KvHandle, KvPinGuard, LagoOidRef};
use crate::types::CloseCode;

#[derive(Default)]
struct Slot {
    pin_count: u32,
    persisted_oid: Option<LagoOidRef>,
    persisted_anima: Option<AnimaIdRef>,
}

// `Default` is intentionally NOT derived here. The auto-derive would
// initialise `next_handle` to `AtomicU64::new(0)` but the cache reserves
// `0` as a sentinel — handles must start at `1`. Construct via
// [`InMemoryKvCache::new`] only.
struct Inner {
    next_handle: AtomicU64,
    by_key: Mutex<HashMap<KvKey, KvHandle>>,
    slots: Mutex<HashMap<KvHandle, Slot>>,
}

/// Process-local [`KvCache`] backed by a `HashMap`. Test-only; not for
/// production. Persistence simulates Lago by minting a synthetic OID.
pub struct InMemoryKvCache {
    inner: Arc<Inner>,
}

impl InMemoryKvCache {
    /// New empty cache.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Inner {
                next_handle: AtomicU64::new(1),
                by_key: Mutex::new(HashMap::new()),
                slots: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Allocate a fresh handle without populating any state. Used by
    /// tests that want a non-empty handle without going through
    /// `lookup` + `populate`. Not part of the public trait.
    #[doc(hidden)]
    #[must_use]
    pub fn allocate_for_test(&self) -> KvHandle {
        let h = KvHandle(self.inner.next_handle.fetch_add(1, Ordering::Relaxed));
        self.inner.slots.lock().unwrap().insert(h, Slot::default());
        h
    }

    /// Current pin count for `handle`. Test-only.
    #[doc(hidden)]
    #[must_use]
    pub fn pin_count(&self, handle: KvHandle) -> u32 {
        self.inner
            .slots
            .lock()
            .unwrap()
            .get(&handle)
            .map_or(0, |s| s.pin_count)
    }

    fn fresh_handle(&self) -> KvHandle {
        let h = KvHandle(self.inner.next_handle.fetch_add(1, Ordering::Relaxed));
        self.inner.slots.lock().unwrap().insert(h, Slot::default());
        h
    }
}

impl KvCache for InMemoryKvCache {
    fn lookup(&self, key: &KvKey) -> Option<KvHandle> {
        self.inner.by_key.lock().unwrap().get(key).copied()
    }

    fn fork(&self, base: KvHandle) -> KvHandle {
        // Real CoW would observe reads from `base` until divergence.
        // The in-mem cache stores nothing useful, so a fresh handle
        // is sufficient for trait-shape tests. Defensively assert in
        // debug builds that the caller is forking a known handle —
        // catches obvious test-setup mistakes.
        debug_assert!(
            self.inner.slots.lock().unwrap().contains_key(&base),
            "fork called with unknown KvHandle({:?})",
            base.0,
        );
        let _ = base;
        self.fresh_handle()
    }

    fn evict(&self, handle: KvHandle) {
        let mut slots = self.inner.slots.lock().unwrap();
        if let Some(slot) = slots.get(&handle)
            && slot.pin_count > 0
        {
            // Pinned — no-op per the trait contract.
            return;
        }
        slots.remove(&handle);
        // Also remove any by_key entries pointing here.
        let mut by_key = self.inner.by_key.lock().unwrap();
        by_key.retain(|_, h| *h != handle);
    }

    fn persist<'a>(
        &'a self,
        handle: KvHandle,
        anima: &'a AnimaIdRef,
    ) -> Pin<Box<dyn Future<Output = Result<LagoOidRef, InferenceError>> + Send + 'a>> {
        Box::pin(async move {
            let mut slots = self.inner.slots.lock().unwrap();
            let Some(slot) = slots.get_mut(&handle) else {
                return Err(InferenceError::backend(
                    CloseCode::KvEvicted,
                    format!("handle {handle:?} not present"),
                ));
            };
            let oid = LagoOidRef(Arc::from(format!(
                "lago:inmem:{}:{:x}",
                anima.as_str(),
                handle.0
            )));
            slot.persisted_oid = Some(oid.clone());
            slot.persisted_anima = Some(anima.clone());
            Ok(oid)
        })
    }

    fn rehydrate<'a>(
        &'a self,
        oid: &'a LagoOidRef,
        anima: &'a AnimaIdRef,
    ) -> Pin<Box<dyn Future<Output = Result<KvHandle, InferenceError>> + Send + 'a>> {
        Box::pin(async move {
            let slots = self.inner.slots.lock().unwrap();
            for (handle, slot) in slots.iter() {
                if slot.persisted_oid.as_ref() == Some(oid) {
                    if slot.persisted_anima.as_ref() != Some(anima) {
                        return Err(InferenceError::backend(
                            CloseCode::AnimaInvalidated,
                            "OID does not belong to this anima",
                        ));
                    }
                    return Ok(*handle);
                }
            }
            Err(InferenceError::backend(
                CloseCode::KvEvicted,
                "oid not present in in-memory cache",
            ))
        })
    }

    fn pin(&self, handle: KvHandle) -> KvPinGuard {
        {
            let mut slots = self.inner.slots.lock().unwrap();
            slots.entry(handle).or_default().pin_count += 1;
        }
        // Capture an Arc<Inner> in the drop closure — guards may outlive
        // the &self borrow used to construct them, so we tie them to the
        // shared inner state instead of the wrapper. Safe Rust only.
        let inner = Arc::clone(&self.inner);
        KvPinGuard {
            on_drop: Box::new(move || {
                let mut slots = inner.slots.lock().unwrap();
                if let Some(slot) = slots.get_mut(&handle) {
                    slot.pin_count = slot.pin_count.saturating_sub(1);
                }
            }),
            handle,
        }
    }
}
