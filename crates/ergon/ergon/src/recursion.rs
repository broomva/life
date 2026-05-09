//! Recursion safety for `spawn_agent` invocations.
//!
//! The substrate that lets agents create / invoke other agents
//! ([`crate::Agent`] / [`crate::AgentSpec`]) is unsafe by default —
//! a misbehaving agent can spawn an infinite chain, blow the token
//! budget, or recurse into itself indefinitely. [`RecursionContext`]
//! is the per-tick guardrail that prevents these failure modes.
//!
//! Per the architecture spec
//! `docs/superpowers/specs/2026-05-09-bro-1006-authored-agents-architecture.md`
//! §5/§6 — these are NON-NEGOTIABLE substrate hardenings shipped
//! together with `spawn_agent`. They are not deferrable.
//!
//! ## What it tracks
//!
//! 1. **Depth** — how many `spawn_agent` calls have been chained
//!    from the workflow tick's root. Default cap: 8.
//! 2. **Invocation stack** — names of agents currently on the call
//!    chain, from root → current frame. Used for **cycle detection**:
//!    `spawn_agent("foo")` from a frame whose stack already contains
//!    `"foo"` is rejected immediately.
//! 3. **Total invocation count** — top-level + all descendants since
//!    the workflow tick started. Cap prevents pathological
//!    fan-out-then-fan-in patterns.
//! 4. **Shared budgets** — token / wall-clock budgets that propagate
//!    down the recursion tree, with `Arc<Atomic*>` so siblings see
//!    consumption in real time.
//!
//! ## Failure semantics
//!
//! Failures from [`RecursionContext::check_can_spawn`] return as
//! typed [`RecursionError`] variants. The intended caller behavior
//! is to convert them into model-visible tool errors (so the parent
//! agent sees them in-band and can adapt) — NOT panics. A spawn
//! failure is a normal failure mode, not a bug.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

use thiserror::Error;

/// Default cap on recursion depth. Tuned high enough for legitimate
/// 3-4 level dispatch patterns (workflow → goal-pursuer → judge →
/// helper), low enough to fail fast on runaway recursion. Override
/// at construction if you need different policy.
pub const DEFAULT_MAX_RECURSION_DEPTH: u32 = 8;

/// Default cap on total agent invocations per workflow tick.
/// Generous for legitimate fan-out workloads (e.g. 50-item scoring
/// panel = 50 spawn_agent calls); strict enough to catch runaway.
pub const DEFAULT_MAX_INVOCATIONS: u32 = 256;

/// Sentinel for "no budget enforced" on the optional shared
/// counters. The default for `RecursionContext::root()` (i.e. when
/// the host runtime hasn't propagated a budget).
pub const UNLIMITED_BUDGET: i64 = i64::MAX;

/// Per-tick recursion guardrail.
///
/// Lives in [`crate::StepCtx`] and propagates through `spawn_agent`
/// invocations: each spawn opens a child context with `depth + 1`
/// and the target spec name appended to the invocation stack. The
/// shared atomic counters (total invocations, token budget,
/// wall-clock budget) are reference-counted and updated by every
/// frame.
///
/// Construct with [`Self::root`] for the workflow body's top-level
/// frame. The runner ([`crate::run_spec`]) opens children
/// automatically; workflow authors should never construct child
/// frames manually.
#[derive(Debug, Clone)]
pub struct RecursionContext {
    /// Current recursion depth. `0` at the workflow tick's root frame.
    pub depth: u32,
    /// Hard cap on depth. Exceeded → [`RecursionError::DepthExceeded`].
    pub max_depth: u32,

    /// Stack of agent spec names invoked from root to current frame.
    /// Used for cycle detection.
    pub invocation_stack: Vec<String>,

    /// Total agent invocations (top-level + all descendants) since
    /// the workflow tick started. Shared via `Arc<AtomicU32>` so
    /// siblings see consumption.
    pub total_invocations: Arc<AtomicU32>,
    /// Hard cap on total invocations. Exceeded →
    /// [`RecursionError::InvocationLimitExceeded`].
    pub max_invocations: u32,

    /// Token budget remaining (model input + output tokens).
    /// Decremented by spawn_agent? No — the autonomous loop emits
    /// usage events; the host runtime is responsible for charging
    /// against this counter. See `arcan-ergon::run_workflow_as_tick`.
    /// `UNLIMITED_BUDGET` means "no enforcement".
    pub token_budget_remaining: Arc<AtomicI64>,

    /// Wall-clock budget remaining in milliseconds. Same enforcement
    /// pattern as `token_budget_remaining`.
    pub wall_clock_budget_remaining_ms: Arc<AtomicI64>,
}

impl RecursionContext {
    /// Construct a root context for a workflow tick. Defaults: depth
    /// 0, max depth [`DEFAULT_MAX_RECURSION_DEPTH`], max invocations
    /// [`DEFAULT_MAX_INVOCATIONS`], no budget enforcement
    /// ([`UNLIMITED_BUDGET`]).
    pub fn root() -> Self {
        Self {
            depth: 0,
            max_depth: DEFAULT_MAX_RECURSION_DEPTH,
            invocation_stack: Vec::new(),
            total_invocations: Arc::new(AtomicU32::new(0)),
            max_invocations: DEFAULT_MAX_INVOCATIONS,
            token_budget_remaining: Arc::new(AtomicI64::new(UNLIMITED_BUDGET)),
            wall_clock_budget_remaining_ms: Arc::new(AtomicI64::new(UNLIMITED_BUDGET)),
        }
    }

    /// Builder: set the max recursion depth.
    #[must_use]
    pub fn with_max_depth(mut self, max_depth: u32) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Builder: set the max invocations per tick.
    #[must_use]
    pub fn with_max_invocations(mut self, max_invocations: u32) -> Self {
        self.max_invocations = max_invocations;
        self
    }

    /// Builder: set the initial token budget (in tokens).
    /// Pass [`UNLIMITED_BUDGET`] to disable enforcement.
    #[must_use]
    pub fn with_token_budget(mut self, tokens: i64) -> Self {
        self.token_budget_remaining = Arc::new(AtomicI64::new(tokens));
        self
    }

    /// Builder: set the initial wall-clock budget (in milliseconds).
    /// Pass [`UNLIMITED_BUDGET`] to disable enforcement.
    #[must_use]
    pub fn with_wall_clock_budget_ms(mut self, ms: i64) -> Self {
        self.wall_clock_budget_remaining_ms = Arc::new(AtomicI64::new(ms));
        self
    }

    /// Decide whether spawning a sub-agent with the given target
    /// name is allowed in the current frame.
    ///
    /// Atomically increments `total_invocations` if the spawn is
    /// allowed (so siblings see the increment immediately). Returns
    /// without incrementing on rejection.
    pub fn check_can_spawn(&self, target_spec_name: &str) -> Result<(), RecursionError> {
        // 1. Depth check.
        if self.depth >= self.max_depth {
            return Err(RecursionError::DepthExceeded {
                depth: self.depth,
                max_depth: self.max_depth,
                attempted: target_spec_name.to_owned(),
            });
        }

        // 2. Cycle check — if the target name is already on the
        // stack, refuse.
        if self.invocation_stack.iter().any(|n| n == target_spec_name) {
            return Err(RecursionError::CycleDetected {
                cycle_target: target_spec_name.to_owned(),
                stack: self.invocation_stack.clone(),
            });
        }

        // 3. Total invocations cap. Use compare-exchange to avoid a
        // race where two siblings both see N-1 and both increment to
        // N when only one should be allowed.
        let mut current = self.total_invocations.load(Ordering::Relaxed);
        loop {
            if current >= self.max_invocations {
                return Err(RecursionError::InvocationLimitExceeded {
                    total: current,
                    max: self.max_invocations,
                    attempted: target_spec_name.to_owned(),
                });
            }
            match self.total_invocations.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }

        // 4. Token budget — soft check (we don't have per-spawn cost
        // estimate yet, so we just refuse if budget is already
        // exhausted from prior consumption).
        if self.token_budget_remaining.load(Ordering::Relaxed) <= 0 {
            return Err(RecursionError::TokenBudgetExhausted {
                attempted: target_spec_name.to_owned(),
            });
        }

        // 5. Wall-clock budget — same pattern.
        if self.wall_clock_budget_remaining_ms.load(Ordering::Relaxed) <= 0 {
            return Err(RecursionError::WallClockBudgetExhausted {
                attempted: target_spec_name.to_owned(),
            });
        }

        Ok(())
    }

    /// Build a child context for a spawned sub-agent. The child
    /// shares the atomic counters (so consumption propagates up) and
    /// inherits caps. Depth is incremented; the target name is
    /// appended to the stack.
    ///
    /// Call this AFTER [`Self::check_can_spawn`] has succeeded.
    pub fn child(&self, target_spec_name: &str) -> Self {
        let mut stack = self.invocation_stack.clone();
        stack.push(target_spec_name.to_owned());
        Self {
            depth: self.depth + 1,
            max_depth: self.max_depth,
            invocation_stack: stack,
            total_invocations: Arc::clone(&self.total_invocations),
            max_invocations: self.max_invocations,
            token_budget_remaining: Arc::clone(&self.token_budget_remaining),
            wall_clock_budget_remaining_ms: Arc::clone(&self.wall_clock_budget_remaining_ms),
        }
    }

    /// Read the current total-invocations counter.
    pub fn total_invocations(&self) -> u32 {
        self.total_invocations.load(Ordering::Relaxed)
    }

    /// Read the current token budget remaining.
    pub fn token_budget(&self) -> i64 {
        self.token_budget_remaining.load(Ordering::Relaxed)
    }

    /// Read the current wall-clock budget remaining (ms).
    pub fn wall_clock_budget_ms(&self) -> i64 {
        self.wall_clock_budget_remaining_ms.load(Ordering::Relaxed)
    }

    /// Charge tokens against the shared budget. Negative `amount`
    /// would refund (we never refund — saturating_sub at 0 floor).
    pub fn charge_tokens(&self, amount: u32) {
        let amt = amount as i64;
        let mut current = self.token_budget_remaining.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_sub(amt).max(0);
            match self.token_budget_remaining.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }

    /// Charge wall-clock milliseconds against the shared budget.
    pub fn charge_wall_clock_ms(&self, ms: u64) {
        let amt = ms as i64;
        let mut current = self.wall_clock_budget_remaining_ms.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_sub(amt).max(0);
            match self.wall_clock_budget_remaining_ms.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }
}

impl Default for RecursionContext {
    fn default() -> Self {
        Self::root()
    }
}

/// Failures returned by [`RecursionContext::check_can_spawn`]. The
/// caller should convert these into model-visible tool errors (e.g.
/// `ToolResult::model_error`) — not panics.
#[derive(Debug, Clone, Error, PartialEq)]
#[non_exhaustive]
pub enum RecursionError {
    #[error(
        "agent recursion depth limit reached (depth={depth}, max={max_depth}); refusing to spawn `{attempted}`"
    )]
    DepthExceeded {
        depth: u32,
        max_depth: u32,
        attempted: String,
    },

    #[error(
        "cycle detected: spawning `{cycle_target}` would re-enter an active frame (stack: {stack:?})"
    )]
    CycleDetected {
        cycle_target: String,
        stack: Vec<String>,
    },

    #[error(
        "agent invocation limit reached (total={total}, max={max}); refusing to spawn `{attempted}`"
    )]
    InvocationLimitExceeded {
        total: u32,
        max: u32,
        attempted: String,
    },

    #[error("token budget exhausted; refusing to spawn `{attempted}`")]
    TokenBudgetExhausted { attempted: String },

    #[error("wall-clock budget exhausted; refusing to spawn `{attempted}`")]
    WallClockBudgetExhausted { attempted: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_starts_at_depth_zero() {
        let ctx = RecursionContext::root();
        assert_eq!(ctx.depth, 0);
        assert!(ctx.invocation_stack.is_empty());
        assert_eq!(ctx.total_invocations(), 0);
    }

    #[test]
    fn child_increments_depth_and_extends_stack() {
        let root = RecursionContext::root();
        root.check_can_spawn("a").expect("ok");
        let a = root.child("a");
        assert_eq!(a.depth, 1);
        assert_eq!(a.invocation_stack, vec!["a".to_string()]);

        a.check_can_spawn("b").expect("ok");
        let b = a.child("b");
        assert_eq!(b.depth, 2);
        assert_eq!(b.invocation_stack, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn depth_limit_blocks_spawn() {
        let mut ctx = RecursionContext::root().with_max_depth(2);
        // depth 0 → spawn child a (depth 1) → spawn b (depth 2) → spawn c (depth 3) blocked
        ctx.check_can_spawn("a").expect("depth 0->1 ok");
        ctx = ctx.child("a");
        ctx.check_can_spawn("b").expect("depth 1->2 ok");
        ctx = ctx.child("b");
        let err = ctx.check_can_spawn("c").expect_err("depth 2->3 must fail");
        assert!(matches!(err, RecursionError::DepthExceeded { .. }));
        assert!(format!("{err}").contains("depth=2"));
    }

    #[test]
    fn cycle_detection_blocks_self_invocation() {
        let root = RecursionContext::root();
        root.check_can_spawn("foo").expect("first ok");
        let foo = root.child("foo");
        let err = foo
            .check_can_spawn("foo")
            .expect_err("recursive self-call must fail");
        assert!(matches!(err, RecursionError::CycleDetected { .. }));
    }

    #[test]
    fn cycle_detection_blocks_indirect_loop() {
        let root = RecursionContext::root();
        root.check_can_spawn("a").expect("ok");
        let a = root.child("a");
        a.check_can_spawn("b").expect("ok");
        let b = a.child("b");
        let err = b.check_can_spawn("a").expect_err("a->b->a cycle must fail");
        if let RecursionError::CycleDetected { stack, .. } = err {
            assert_eq!(stack, vec!["a".to_string(), "b".to_string()]);
        } else {
            panic!("expected CycleDetected");
        }
    }

    #[test]
    fn invocation_limit_blocks_after_n() {
        let ctx = RecursionContext::root().with_max_invocations(3);
        ctx.check_can_spawn("a").expect("1");
        ctx.check_can_spawn("b").expect("2");
        ctx.check_can_spawn("c").expect("3");
        let err = ctx.check_can_spawn("d").expect_err("4 must fail");
        assert!(matches!(
            err,
            RecursionError::InvocationLimitExceeded { .. }
        ));
    }

    #[test]
    fn token_budget_exhaustion_blocks_spawn() {
        let ctx = RecursionContext::root().with_token_budget(100);
        ctx.charge_tokens(100);
        let err = ctx.check_can_spawn("a").expect_err("zero budget must fail");
        assert!(matches!(err, RecursionError::TokenBudgetExhausted { .. }));
    }

    #[test]
    fn wall_clock_budget_exhaustion_blocks_spawn() {
        let ctx = RecursionContext::root().with_wall_clock_budget_ms(1000);
        ctx.charge_wall_clock_ms(1000);
        let err = ctx.check_can_spawn("a").expect_err("zero budget must fail");
        assert!(matches!(
            err,
            RecursionError::WallClockBudgetExhausted { .. }
        ));
    }

    #[test]
    fn budgets_propagate_to_children() {
        let root = RecursionContext::root().with_token_budget(1000);
        root.check_can_spawn("a").expect("ok");
        let a = root.child("a");
        // Charge from child — root sees the consumption.
        a.charge_tokens(400);
        assert_eq!(root.token_budget(), 600);
        assert_eq!(a.token_budget(), 600);

        // Charge from root — child sees it.
        root.charge_tokens(500);
        assert_eq!(root.token_budget(), 100);
        assert_eq!(a.token_budget(), 100);
    }

    #[test]
    fn invocation_count_propagates_to_children() {
        let root = RecursionContext::root();
        root.check_can_spawn("a").expect("ok");
        // Child sees the same atomic counter.
        let a = root.child("a");
        a.check_can_spawn("b").expect("ok");
        // Both see 2 (one increment per check_can_spawn).
        assert_eq!(root.total_invocations(), 2);
        assert_eq!(a.total_invocations(), 2);
    }

    #[test]
    fn parallel_siblings_share_invocation_count() {
        // Simulate concurrent fan-out from a single parent.
        let root = RecursionContext::root().with_max_invocations(5);
        let handles: Vec<_> = (0..5)
            .map(|i| {
                let r = root.clone();
                std::thread::spawn(move || r.check_can_spawn(&format!("agent-{i}")))
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let oks = results.iter().filter(|r| r.is_ok()).count();
        let errs = results.iter().filter(|r| r.is_err()).count();
        assert_eq!(oks + errs, 5);
        // All 5 should fit since cap is 5.
        assert_eq!(oks, 5);
        // 6th one should fail.
        assert!(matches!(
            root.check_can_spawn("agent-6"),
            Err(RecursionError::InvocationLimitExceeded { .. })
        ));
    }

    #[test]
    fn unlimited_budget_never_exhausts() {
        let ctx = RecursionContext::root();
        // Default is unlimited.
        assert_eq!(ctx.token_budget(), UNLIMITED_BUDGET);
        ctx.charge_tokens(1_000_000);
        // Still within margin.
        assert!(ctx.token_budget() > i64::MAX / 2);
        // Spawn still succeeds.
        ctx.check_can_spawn("a").expect("ok");
    }
}
