//! Token-bucket rate limiter (Spec C₃ §7).
//!
//! Sub-phase D (D1) implements per-user + per-IP token-bucket rate
//! limiting. Per master spec §L12 #10 + Spec C₃ §7.2:
//!
//! - **Per-user bucket** keyed by `Tier1Claims.user_id`. Default
//!   capacity 60 req, refill 60 req/sec (the `free` tier defaults).
//!   Operators raise per-tier via [`OverrideStore`] (Sub-phase D D2).
//! - **Per-IP bucket** for pre-auth + IP-shared workplaces. Default
//!   capacity 60 req, refill 60 req/min.
//! - **Eviction**: LRU with 10k bucket cap so memory stays bounded
//!   even under a burst of unique users / IPs.
//!
//! On overage the limiter returns [`RateLimitDecision::Reject`] and
//! the middleware translates this to:
//! - HTTP / gRPC: `Status::resource_exhausted("rate limited")` →
//!   tonic emits `gRPC code 8` (RESOURCE_EXHAUSTED) which our
//!   [`crate::services::ws::map_status_to_close`] already maps to
//!   `CloseReason::RateLimit` (4001).
//! - WS upgrade: `429 Too Many Requests`.
//!
//! Per the prompt's hard rule: rate-limit returns
//! `Status::resource_exhausted` (NOT `unavailable`) — the two have
//! different gRPC semantics and only `resource_exhausted` maps cleanly
//! to WS close code 4001 via the existing close-code mapper.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use crate::config::RateLimitConfig;

/// Default LRU eviction cap. Memory bound: ~150 bytes per bucket
/// gives ~1.5 MB at full capacity. Configurable via
/// [`TokenBucketLimiter::with_max_buckets`].
pub const DEFAULT_MAX_BUCKETS: usize = 10_000;

/// One token-bucket entry. Fixed-point arithmetic (`tokens × 1000`)
/// lets the refill computation stay integer-only inside the hot path.
#[derive(Debug, Clone)]
struct Bucket {
    /// Maximum tokens this bucket can hold. Bursts above this are
    /// rejected even if the time-since-last-refill would otherwise
    /// allow them.
    capacity: u32,
    /// Refill rate in tokens × 1000 per second. Stored as
    /// fixed-point so a 60 req/min bucket (1.0 token/sec) doesn't
    /// suffer from integer truncation on per-tick refills.
    refill_per_sec_milli: u64,
    /// Current tokens × 1000.
    tokens_milli: i64,
    /// Last time this bucket's tokens were refilled.
    last_refill: Instant,
    /// LRU touch-stamp — when this bucket was last consulted (hit or
    /// miss). The eviction sweeper drops the oldest buckets first.
    last_touched: Instant,
}

impl Bucket {
    fn new(capacity: u32, refill_per_sec: u32, now: Instant) -> Self {
        Self {
            capacity,
            refill_per_sec_milli: u64::from(refill_per_sec) * 1000,
            tokens_milli: i64::from(capacity) * 1000,
            last_refill: now,
            last_touched: now,
        }
    }

    fn new_per_minute(capacity: u32, refill_per_min: u32, now: Instant) -> Self {
        // Per-minute → per-second: refill_per_sec_milli =
        // refill_per_min * 1000 / 60. We keep the fixed-point
        // representation precise for sub-1-token/sec rates.
        let per_sec_milli = u64::from(refill_per_min) * 1000 / 60;
        Self {
            capacity,
            refill_per_sec_milli: per_sec_milli,
            tokens_milli: i64::from(capacity) * 1000,
            last_refill: now,
            last_touched: now,
        }
    }

    /// Refill the bucket up to `capacity` based on time elapsed since
    /// `last_refill`. Pure-time-based refill — no per-request
    /// off-the-clock work, so the bucket's behaviour is testable with
    /// mocked `Instant`s in unit tests via `try_acquire_at`.
    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last_refill);
        // Tokens to add = elapsed_secs × refill_per_sec_milli (fixed
        // point). Use millis to avoid float for sub-second elapsed.
        let elapsed_ms = elapsed.as_millis() as u64;
        let added_milli = elapsed_ms.saturating_mul(self.refill_per_sec_milli) / 1000;
        let cap_milli = i64::from(self.capacity) * 1000;
        self.tokens_milli = self
            .tokens_milli
            .saturating_add(added_milli as i64)
            .min(cap_milli);
        self.last_refill = now;
    }

    /// Try to consume one token. Returns `true` on success (token
    /// available + decremented), `false` on rejection (empty bucket).
    fn try_consume(&mut self, now: Instant) -> bool {
        self.refill(now);
        self.last_touched = now;
        if self.tokens_milli >= 1000 {
            self.tokens_milli -= 1000;
            true
        } else {
            false
        }
    }
}

/// Decision returned by [`TokenBucketLimiter::check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RateLimitDecision {
    /// Token consumed — request may proceed.
    Allow,
    /// Per-user bucket empty — middleware should return
    /// `Status::resource_exhausted("rate_limit:per_user")` (mapped to
    /// WS close code 4001 via the existing
    /// [`crate::services::ws::map_status_to_close`] helper).
    RejectPerUser,
    /// Per-IP bucket empty.
    RejectPerIp,
}

impl RateLimitDecision {
    /// `true` when the request must be rejected.
    pub fn is_reject(self) -> bool {
        !matches!(self, RateLimitDecision::Allow)
    }

    /// String reason for tonic Status messages + WS close-frame
    /// payloads.
    pub fn reason(self) -> &'static str {
        match self {
            RateLimitDecision::Allow => "ok",
            RateLimitDecision::RejectPerUser => "rate_limit:per_user",
            RateLimitDecision::RejectPerIp => "rate_limit:per_ip",
        }
    }
}

/// Override entry for [`TokenBucketLimiter`]. Operators can raise a
/// specific user's budget at runtime via the admin plane (Sub-phase D
/// D2). Overrides are in-process — restart resets to the configured
/// defaults.
#[derive(Debug, Clone, Copy)]
pub struct RateLimitOverride {
    pub capacity: u32,
    pub refill_per_sec: u32,
}

/// Per-user + per-IP token-bucket limiter.
///
/// Both maps are bounded via LRU eviction once they exceed the
/// configured `max_buckets` cap. Concurrent access uses a single
/// `parking_lot::Mutex` per map — fine-grained per-key sharding is a
/// future optimisation if profiling shows the global lock is hot.
#[non_exhaustive]
pub struct TokenBucketLimiter {
    inner: Arc<Inner>,
}

struct Inner {
    user_capacity: u32,
    user_refill_per_sec: u32,
    ip_capacity: u32,
    ip_refill_per_min: u32,
    max_buckets: usize,
    user_buckets: Mutex<HashMap<String, Bucket>>,
    ip_buckets: Mutex<HashMap<IpAddr, Bucket>>,
    overrides: Mutex<HashMap<String, RateLimitOverride>>,
}

impl TokenBucketLimiter {
    /// Build a limiter from a [`RateLimitConfig`]. The config carries
    /// the master-spec defaults (60 req/sec/user; 60 req/min/IP).
    pub fn from_config(cfg: &RateLimitConfig) -> Self {
        Self::new(
            cfg.per_user_capacity,
            cfg.per_user_refill_per_sec,
            cfg.per_ip_capacity,
            cfg.per_ip_refill_per_min,
            DEFAULT_MAX_BUCKETS,
        )
    }

    /// Build a limiter with explicit defaults. Used by tests.
    pub fn new(
        user_capacity: u32,
        user_refill_per_sec: u32,
        ip_capacity: u32,
        ip_refill_per_min: u32,
        max_buckets: usize,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                user_capacity,
                user_refill_per_sec,
                ip_capacity,
                ip_refill_per_min,
                max_buckets,
                user_buckets: Mutex::new(HashMap::new()),
                ip_buckets: Mutex::new(HashMap::new()),
                overrides: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Override the configured per-user budget for `user_id`. Used
    /// by the admin plane (Sub-phase D D2) — operators can throttle
    /// abusive callers without restarting the daemon.
    pub fn set_user_override(&self, user_id: &str, ov: RateLimitOverride) {
        self.inner.overrides.lock().insert(user_id.to_string(), ov);
        // Force a fresh bucket build on the next `check` so the override
        // takes effect immediately (rather than after the existing
        // bucket drains).
        self.inner.user_buckets.lock().remove(user_id);
    }

    /// Remove a per-user override. The user reverts to the default
    /// budget on the next request.
    pub fn clear_user_override(&self, user_id: &str) -> Option<RateLimitOverride> {
        let removed = self.inner.overrides.lock().remove(user_id);
        // Same eager-rebuild rationale as `set_user_override`.
        self.inner.user_buckets.lock().remove(user_id);
        removed
    }

    /// List all active per-user overrides. Used by the admin plane
    /// `RateLimit_List` RPC.
    pub fn list_user_overrides(&self) -> Vec<(String, RateLimitOverride)> {
        self.inner
            .overrides
            .lock()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// Cheap clone — returns a handle pointing at the same internal
    /// state. Used to share the limiter across the auth Layer +
    /// admin handlers.
    pub fn handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Decide whether `user_id` (post-auth) and `ip` may proceed.
    /// Both buckets are checked; if either is empty the request is
    /// rejected. Per-IP rejection takes precedence (it's the cheaper
    /// dimension to enforce against pre-auth probes).
    pub fn check(&self, user_id: &str, ip: IpAddr) -> RateLimitDecision {
        self.check_at(user_id, ip, Instant::now())
    }

    /// Same as [`Self::check`] but with an explicit `now`. Test
    /// helper.
    pub fn check_at(&self, user_id: &str, ip: IpAddr, now: Instant) -> RateLimitDecision {
        // Per-IP first — keeps abuse defence applied even if a
        // forged user_id slips past auth (defence in depth).
        if !self.try_consume_ip(ip, now) {
            return RateLimitDecision::RejectPerIp;
        }
        if !self.try_consume_user(user_id, now) {
            return RateLimitDecision::RejectPerUser;
        }
        RateLimitDecision::Allow
    }

    /// Pre-auth IP-only check. Used for `/healthz` over the public
    /// surface (the gateway exposes `/healthz` without auth per Spec
    /// C₃ §3.5) — the IP bucket still applies.
    pub fn check_ip_only(&self, ip: IpAddr) -> RateLimitDecision {
        if !self.try_consume_ip(ip, Instant::now()) {
            return RateLimitDecision::RejectPerIp;
        }
        RateLimitDecision::Allow
    }

    fn try_consume_user(&self, user_id: &str, now: Instant) -> bool {
        let mut buckets = self.inner.user_buckets.lock();
        Self::evict_if_needed(&mut buckets, self.inner.max_buckets, now);
        let bucket = buckets.entry(user_id.to_string()).or_insert_with(|| {
            match self.inner.overrides.lock().get(user_id) {
                Some(ov) => Bucket::new(ov.capacity, ov.refill_per_sec, now),
                None => Bucket::new(
                    self.inner.user_capacity,
                    self.inner.user_refill_per_sec,
                    now,
                ),
            }
        });
        bucket.try_consume(now)
    }

    fn try_consume_ip(&self, ip: IpAddr, now: Instant) -> bool {
        let mut buckets = self.inner.ip_buckets.lock();
        Self::evict_if_needed(&mut buckets, self.inner.max_buckets, now);
        let bucket = buckets.entry(ip).or_insert_with(|| {
            Bucket::new_per_minute(self.inner.ip_capacity, self.inner.ip_refill_per_min, now)
        });
        bucket.try_consume(now)
    }

    /// LRU eviction sweeper. When the map size exceeds `cap`, we drop
    /// the K oldest entries until the size is back to `cap × 0.9`.
    /// The 10% buffer prevents thrashing when the map is near the
    /// cap.
    fn evict_if_needed<K>(map: &mut HashMap<K, Bucket>, cap: usize, _now: Instant)
    where
        K: std::hash::Hash + Eq + Clone,
    {
        if map.len() <= cap {
            return;
        }
        let target = cap * 9 / 10;
        let to_drop = map.len() - target;
        // Collect (key, last_touched) and pick the `to_drop` oldest.
        let mut entries: Vec<(K, Instant)> = map
            .iter()
            .map(|(k, v)| (k.clone(), v.last_touched))
            .collect();
        entries.sort_by_key(|(_, t)| *t);
        for (k, _) in entries.into_iter().take(to_drop) {
            map.remove(&k);
        }
    }

    /// Test helper: how many user buckets are tracked.
    #[doc(hidden)]
    pub fn user_bucket_count(&self) -> usize {
        self.inner.user_buckets.lock().len()
    }

    /// Test helper: how many IP buckets are tracked.
    #[doc(hidden)]
    pub fn ip_bucket_count(&self) -> usize {
        self.inner.ip_buckets.lock().len()
    }
}

impl Clone for TokenBucketLimiter {
    fn clone(&self) -> Self {
        self.handle()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn ip_v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn first_request_allowed_for_fresh_user() {
        let lim = TokenBucketLimiter::new(60, 60, 600, 60, 100);
        let d = lim.check("alice", ip_v4(127, 0, 0, 1));
        assert_eq!(d, RateLimitDecision::Allow);
    }

    #[test]
    fn over_capacity_rejected_for_user() {
        // Capacity 5, refill 0/sec → 6th request in <1s rejected.
        let lim = TokenBucketLimiter::new(5, 0, 1000, 60, 100);
        let now = Instant::now();
        for _ in 0..5 {
            assert_eq!(
                lim.check_at("bob", ip_v4(10, 0, 0, 1), now),
                RateLimitDecision::Allow
            );
        }
        assert_eq!(
            lim.check_at("bob", ip_v4(10, 0, 0, 1), now),
            RateLimitDecision::RejectPerUser
        );
    }

    #[test]
    fn refill_after_time_elapses() {
        // Capacity 5, refill 5/sec → bucket is full again 1s after
        // being drained.
        let lim = TokenBucketLimiter::new(5, 5, 1000, 60, 100);
        let t0 = Instant::now();
        for _ in 0..5 {
            lim.check_at("carol", ip_v4(192, 168, 0, 1), t0);
        }
        // Empty
        assert_eq!(
            lim.check_at("carol", ip_v4(192, 168, 0, 1), t0),
            RateLimitDecision::RejectPerUser
        );
        // 1s later — refill 5 tokens, so 5 more requests succeed.
        let t1 = t0 + Duration::from_secs(1);
        for _ in 0..5 {
            assert_eq!(
                lim.check_at("carol", ip_v4(192, 168, 0, 1), t1),
                RateLimitDecision::Allow
            );
        }
        assert_eq!(
            lim.check_at("carol", ip_v4(192, 168, 0, 1), t1),
            RateLimitDecision::RejectPerUser
        );
    }

    #[test]
    fn per_ip_bucket_separate_from_per_user() {
        // IP gets 2 tokens; users have 100. Same IP but different
        // users → second user's first request rejects on IP bucket.
        let lim = TokenBucketLimiter::new(100, 0, 2, 0, 100);
        let now = Instant::now();
        let ip = ip_v4(1, 2, 3, 4);
        assert_eq!(lim.check_at("u1", ip, now), RateLimitDecision::Allow);
        assert_eq!(lim.check_at("u2", ip, now), RateLimitDecision::Allow);
        // Third request — IP bucket empty (regardless of user).
        assert_eq!(lim.check_at("u3", ip, now), RateLimitDecision::RejectPerIp);
    }

    #[test]
    fn user_override_raises_capacity() {
        let lim = TokenBucketLimiter::new(2, 0, 1000, 60, 100);
        // Default capacity of 2 → 3rd request rejects.
        let now = Instant::now();
        let ip = ip_v4(127, 0, 0, 1);
        for _ in 0..2 {
            lim.check_at("vip", ip, now);
        }
        assert_eq!(
            lim.check_at("vip", ip, now),
            RateLimitDecision::RejectPerUser
        );

        // Operator override → capacity 100; bucket is rebuilt fresh.
        lim.set_user_override(
            "vip",
            RateLimitOverride {
                capacity: 100,
                refill_per_sec: 0,
            },
        );
        let later = now + Duration::from_secs(1);
        // Fresh bucket has 100 tokens; the next 50 calls all succeed.
        for _ in 0..50 {
            assert_eq!(lim.check_at("vip", ip, later), RateLimitDecision::Allow);
        }
    }

    #[test]
    fn user_override_clear_reverts_to_default() {
        let lim = TokenBucketLimiter::new(2, 0, 1000, 60, 100);
        lim.set_user_override(
            "vip",
            RateLimitOverride {
                capacity: 100,
                refill_per_sec: 0,
            },
        );
        let removed = lim.clear_user_override("vip");
        assert!(removed.is_some());
        // After clear, default capacity of 2 applies again.
        let now = Instant::now();
        let ip = ip_v4(127, 0, 0, 1);
        lim.check_at("vip", ip, now);
        lim.check_at("vip", ip, now);
        assert_eq!(
            lim.check_at("vip", ip, now),
            RateLimitDecision::RejectPerUser
        );
    }

    #[test]
    fn lru_eviction_bounds_memory() {
        // Cap 10 → after 20 unique users the map is shrunk to ≤10.
        let lim = TokenBucketLimiter::new(60, 60, 1000, 60, 10);
        let now = Instant::now();
        for i in 0..20 {
            let user = format!("u{i}");
            lim.check_at(
                &user,
                ip_v4(127, 0, 0, 1),
                now + Duration::from_millis(i as u64),
            );
        }
        assert!(
            lim.user_bucket_count() <= 10,
            "LRU must bound user buckets to ≤10; observed {}",
            lim.user_bucket_count()
        );
    }

    #[test]
    fn per_ip_rejection_takes_precedence() {
        // Per-IP cap is hit before per-user — the request is rejected
        // with the IP reason. This matches the auth-Layer ordering: IP
        // bucket gates pre-auth + post-auth so an attacker who forges
        // a fresh user_id every request still hits the IP wall.
        let lim = TokenBucketLimiter::new(100, 0, 1, 0, 100);
        let now = Instant::now();
        let ip = ip_v4(8, 8, 8, 8);
        // First request consumes both buckets — Allow.
        assert_eq!(lim.check_at("a", ip, now), RateLimitDecision::Allow);
        // Second request — IP bucket empty, user bucket still has 99.
        // Decision MUST be RejectPerIp, NOT RejectPerUser.
        assert_eq!(lim.check_at("b", ip, now), RateLimitDecision::RejectPerIp);
    }

    #[test]
    fn decision_reason_strings_match_expected() {
        // Sanity-check the reason strings we surface to clients via
        // tonic Status messages and WS close frames.
        assert_eq!(RateLimitDecision::Allow.reason(), "ok");
        assert_eq!(
            RateLimitDecision::RejectPerUser.reason(),
            "rate_limit:per_user"
        );
        assert_eq!(RateLimitDecision::RejectPerIp.reason(), "rate_limit:per_ip");
    }

    #[test]
    fn fixed_point_refill_handles_sub_token_per_sec() {
        // 60 req/min IP bucket → 1 token/sec. The fixed-point
        // arithmetic must let a 500 ms wait add half a token (which
        // doesn't enable a request yet) and a full second add one
        // token (which does).
        let lim = TokenBucketLimiter::new(1000, 1000, 1, 60, 100);
        let t0 = Instant::now();
        let ip = ip_v4(1, 1, 1, 1);
        // Drain.
        assert_eq!(lim.check_at("u", ip, t0), RateLimitDecision::Allow);
        assert_eq!(lim.check_at("u", ip, t0), RateLimitDecision::RejectPerIp);

        // 500 ms — still under capacity (refill < 1 token).
        let t_half = t0 + Duration::from_millis(500);
        assert_eq!(
            lim.check_at("u", ip, t_half),
            RateLimitDecision::RejectPerIp
        );

        // 1 second — full token available.
        let t_full = t0 + Duration::from_millis(1000);
        assert_eq!(lim.check_at("u", ip, t_full), RateLimitDecision::Allow);
    }
}
