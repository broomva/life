//! Revocation check (Spec D D-Sub-E).
//!
//! Spec D §"Event additions" defines `anima.identity_revoked { did,
//! reason, revoked_at }`. After this event lands in the Lago journal,
//! no signature signed by the revoked DID is accepted regardless of
//! seq. Used for stolen devices, compromised passkeys, or end-of-life
//! identities.
//!
//! The trait surface is in [`crate::rotation::JournalResolver`]; this
//! module adds an in-process cache so verifiers don't hammer Lago on
//! every signature check.
//!
//! ## Cache semantics
//!
//! - **Negative answer cached for `cache_ttl`**. The common case is
//!   "DID is not revoked"; we cache that answer with a TTL so a hot
//!   identity doesn't pay the journal lookup on every JWT.
//! - **Positive answer cached forever**. Once a DID is revoked it
//!   stays revoked — `anima.identity_revoked` is one-way, the
//!   journal does not emit "un-revoked" events.
//! - **Default TTL**: 60 seconds. Operators can tune via
//!   [`RevocationCache::with_ttl`] for environments where revocations
//!   need to propagate faster (e.g. incident response).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anima_core::error::AnimaResult;
use parking_lot::RwLock;

use crate::rotation::JournalResolver;

/// Default cache TTL for negative ("not revoked") answers.
pub const DEFAULT_REVOCATION_CACHE_TTL: Duration = Duration::from_secs(60);

/// In-process cache over a [`JournalResolver`].
///
/// Wraps a resolver + per-DID expiry timestamps. Each entry stores
/// `(is_revoked, expires_at)`; entries that have expired trigger a
/// fresh resolver call. Positive answers (`is_revoked == true`) are
/// stored with a far-future expiry so they never need re-checking.
#[derive(Clone)]
pub struct RevocationCache {
    inner: Arc<RwLock<HashMap<String, CacheEntry>>>,
    ttl: Duration,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    revoked: bool,
    /// `None` for positive answers (never expire); `Some` for negative
    /// answers (expire at the wall-clock deadline).
    expires_at: Option<Instant>,
}

impl Default for RevocationCache {
    fn default() -> Self {
        Self::with_ttl(DEFAULT_REVOCATION_CACHE_TTL)
    }
}

impl std::fmt::Debug for RevocationCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevocationCache")
            .field("ttl", &self.ttl)
            .finish()
    }
}

impl RevocationCache {
    /// Construct a fresh cache with the default TTL.
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_REVOCATION_CACHE_TTL)
    }

    /// Construct a fresh cache with a custom TTL for negative answers.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Look up `did` in the cache; returns the cached result if
    /// fresh, otherwise calls the resolver and stores the answer.
    pub async fn check(&self, did: &str, resolver: &dyn JournalResolver) -> AnimaResult<bool> {
        let now = Instant::now();
        // Fast path — read the cache and see if we have a fresh entry.
        if let Some(entry) = self.inner.read().get(did).cloned() {
            let expired = entry.expires_at.is_some_and(|deadline| now >= deadline);
            if !expired {
                return Ok(entry.revoked);
            }
        }
        // Slow path — resolver call.
        let revoked_seq = resolver.revocation_event_for(did).await?;
        let revoked = revoked_seq.is_some();
        let expires_at = if revoked {
            // Positive answer — never expire.
            None
        } else {
            Some(now + self.ttl)
        };
        self.inner.write().insert(
            did.to_string(),
            CacheEntry {
                revoked,
                expires_at,
            },
        );
        Ok(revoked)
    }

    /// Clear the cache for a single DID. Used after a manual
    /// revocation event is written to the journal so subsequent
    /// checks pick up the new state immediately rather than waiting
    /// for the negative-answer TTL to elapse.
    pub fn invalidate(&self, did: &str) {
        self.inner.write().remove(did);
    }

    /// Clear the entire cache. Used in tests + on JWKS rotation.
    pub fn clear(&self) {
        self.inner.write().clear();
    }
}

/// Convenience wrapper: check whether `did` is revoked, going through
/// `cache` if provided or hitting `resolver` directly otherwise.
///
/// Most production callers use [`lago_auth::agent_jwt::verify_jwt`]
/// which holds the cache for the lifetime of the auth middleware.
/// This free function is kept for tests and one-shot revocation
/// queries.
pub async fn is_revoked(
    did: &str,
    resolver: &dyn JournalResolver,
    cache: Option<&RevocationCache>,
) -> AnimaResult<bool> {
    match cache {
        Some(cache) => cache.check(did, resolver).await,
        None => Ok(resolver.revocation_event_for(did).await?.is_some()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rotation::RotationChainQuery;
    use anima_core::identity_document::DidRotation;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Test resolver that counts how many times each method is called.
    struct CountingResolver {
        revoked: Vec<String>,
        rotation_calls: AtomicUsize,
        revocation_calls: AtomicUsize,
    }

    impl CountingResolver {
        fn with_revoked(revoked: Vec<&str>) -> Self {
            Self {
                revoked: revoked.into_iter().map(String::from).collect(),
                rotation_calls: AtomicUsize::new(0),
                revocation_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl JournalResolver for CountingResolver {
        async fn rotation_events_for(
            &self,
            _q: RotationChainQuery<'_>,
        ) -> AnimaResult<Vec<DidRotation>> {
            self.rotation_calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        }

        async fn revocation_event_for(&self, did: &str) -> AnimaResult<Option<u64>> {
            self.revocation_calls.fetch_add(1, Ordering::SeqCst);
            if self.revoked.iter().any(|d| d == did) {
                Ok(Some(42))
            } else {
                Ok(None)
            }
        }
    }

    #[tokio::test]
    async fn unrevoked_did_returns_false() {
        let resolver = CountingResolver::with_revoked(vec![]);
        let cache = RevocationCache::new();
        assert!(!cache.check("did:key:zDnFresh", &resolver).await.unwrap());
    }

    #[tokio::test]
    async fn revoked_did_returns_true() {
        let resolver = CountingResolver::with_revoked(vec!["did:key:zDnBad"]);
        let cache = RevocationCache::new();
        assert!(cache.check("did:key:zDnBad", &resolver).await.unwrap());
    }

    #[tokio::test]
    async fn negative_answer_is_cached_within_ttl() {
        let resolver = CountingResolver::with_revoked(vec![]);
        let cache = RevocationCache::with_ttl(Duration::from_secs(60));
        let _ = cache.check("did:key:zDnA", &resolver).await.unwrap();
        let _ = cache.check("did:key:zDnA", &resolver).await.unwrap();
        let _ = cache.check("did:key:zDnA", &resolver).await.unwrap();
        assert_eq!(
            resolver.revocation_calls.load(Ordering::SeqCst),
            1,
            "TTL-cached negative answer should hit resolver only once"
        );
    }

    #[tokio::test]
    async fn positive_answer_is_cached_forever() {
        let resolver = CountingResolver::with_revoked(vec!["did:key:zDnGone"]);
        let cache = RevocationCache::with_ttl(Duration::from_millis(1));
        // Even after the TTL window, the positive answer stays cached.
        assert!(cache.check("did:key:zDnGone", &resolver).await.unwrap());
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(cache.check("did:key:zDnGone", &resolver).await.unwrap());
        assert_eq!(
            resolver.revocation_calls.load(Ordering::SeqCst),
            1,
            "positive answer should not re-resolve after TTL"
        );
    }

    #[tokio::test]
    async fn invalidate_forces_resolver_recall() {
        let resolver = CountingResolver::with_revoked(vec![]);
        let cache = RevocationCache::with_ttl(Duration::from_secs(60));
        assert!(!cache.check("did:key:zDnX", &resolver).await.unwrap());
        cache.invalidate("did:key:zDnX");
        assert!(!cache.check("did:key:zDnX", &resolver).await.unwrap());
        assert_eq!(
            resolver.revocation_calls.load(Ordering::SeqCst),
            2,
            "invalidate should drop the cached entry"
        );
    }

    #[tokio::test]
    async fn is_revoked_helper_works_without_cache() {
        let resolver = CountingResolver::with_revoked(vec!["did:key:zDnGone"]);
        let outcome = is_revoked("did:key:zDnGone", &resolver, None)
            .await
            .unwrap();
        assert!(outcome);
    }

    #[tokio::test]
    async fn is_revoked_helper_uses_cache_when_provided() {
        let resolver = CountingResolver::with_revoked(vec![]);
        let cache = RevocationCache::new();
        let _ = is_revoked("did:key:zDnSafe", &resolver, Some(&cache))
            .await
            .unwrap();
        let _ = is_revoked("did:key:zDnSafe", &resolver, Some(&cache))
            .await
            .unwrap();
        assert_eq!(resolver.revocation_calls.load(Ordering::SeqCst), 1);
    }
}
