//! Chaos test (Sub-phase E item #6 — chaos #2): JWKS outage →
//! `FlightCoalescer` propagates the failure cohort-wide without a
//! stampede.
//!
//! Spec C₃ §5.4 + master spec §L4 invariant 1: the gateway must
//! verify Tier-1 tokens against the upstream JWKS. When the upstream
//! is unreachable AND no kid is cached, every concurrent in-flight
//! request observes the SAME error — the coalescer admits exactly
//! one fetcher who runs the upstream call; everyone else waits on a
//! condvar and reads the cohort's outcome from `last_error`.
//!
//! Without single-flight, 100 concurrent requests during a kid-miss
//! event produce 100 upstream HTTP fetches → Vercel rate-limits the
//! gateway → authn breaks for everyone, not just the cohort.

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::sync::Arc;

use lifegw::auth::jwks::{JwksCache, JwksCacheConfig, JwksDoc, JwksSource};
use tempfile::TempDir;

#[test]
fn jwks_outage_propagates_to_every_cohort_member() {
    // Synthetic outage: point the cache at a non-existent file path.
    // Every refetch returns the same "file not found" error.
    let dir = TempDir::new().expect("tempdir");
    let missing = dir.path().join("never-exists.json");
    let cfg = JwksCacheConfig::new(JwksSource::File(missing), "lifegw", "https://broomva.tech");
    let cache = Arc::new(JwksCache::new(cfg));

    let n = 32;
    let barrier = Arc::new(std::sync::Barrier::new(n));
    let mut handles = Vec::with_capacity(n);
    for _ in 0..n {
        let c = Arc::clone(&cache);
        let b = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            b.wait();
            c.force_refetch()
        }));
    }
    let mut errors = 0;
    for h in handles {
        if h.join().expect("thread join").is_err() {
            errors += 1;
        }
    }
    // Every cohort member surfaces the failure — none silently slip
    // through with a stale cached entry.
    assert_eq!(
        errors, n,
        "every concurrent refetch must surface the upstream outage error"
    );
}

#[test]
fn jwks_outage_recovery_succeeds_once_upstream_returns() {
    // Start with a missing file, then create it with a valid JWKS body.
    // Subsequent refetches must succeed (the coalescer doesn't cache
    // failures past the cohort boundary).
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("jwks.json");
    let cfg = JwksCacheConfig::new(
        JwksSource::File(path.clone()),
        "lifegw",
        "https://broomva.tech",
    );
    let cache = Arc::new(JwksCache::new(cfg));

    // First refetch: file missing → error.
    assert!(
        cache.force_refetch().is_err(),
        "missing file must error first time"
    );

    // Heal: write a valid (empty) JWKS file.
    let empty = serde_json::to_string(&JwksDoc::default()).expect("serialize");
    std::fs::write(&path, empty).expect("write jwks");

    // Second refetch: succeeds.
    cache
        .force_refetch()
        .expect("post-recovery refetch must succeed");
}
