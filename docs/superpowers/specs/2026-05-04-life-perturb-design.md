# life-perturb — Controlled Perturbation Injection for RCS λ̂ Validation

**Status:** v0.1-L0 IN PROGRESS — design + scaffold landed in PR #1088, single-kind L0 injector + autonomic-derived V_0 probe + simulation harness landing in follow-up PR.
**Date:** 2026-05-04
**Owner:** Carlos Escobar
**Linear:** [BRO-947](https://linear.app/broomva/issue/BRO-947)
**Parent:** BRO-944 — RCS thesis empirical validation
**Depends on:** life#802 (StabilityBudget / MarginEstimator), life#804 (RcsObserver)
**Reading order:** read [`research/rcs/microrcs/THESIS_VALIDATION.md`](../../../../research/rcs/microrcs/THESIS_VALIDATION.md) §"Why λ̂ doesn't match paper" first; the gap motivates this entire crate.

## 1. Problem

The Recursive Controlled Systems (RCS) paper (`research/rcs/papers/p0-foundations/main.tex`, Theorem 1) proves each level is exponentially stable iff its stability margin

```
λ_i = γ_i − L_θ_i·ρ_i − L_d_i·η_i − β_i·τ̄_i − ln(ν_i)/τ_{a,i}
```

is strictly positive, with canonical values from `research/rcs/data/parameters.toml`:

| Level | λ_i (paper) | Mirror in Rust |
|-------|-------------|----------------|
| L0    | 1.4554      | `crates/autonomic/autonomic-core/data/rcs-parameters.toml` |
| L1    | 0.4115      | (same)                                                     |
| L2    | 0.0693      | (same)                                                     |
| L3    | 0.0064      | (same)                                                     |

These λ_i are **exponential decay rates of perturbations** in a Lyapunov function V_k. Empirically (microRCS, microgrid) we have measured something different: a regression slope on stationary V_k(t) traces with no controlled perturbation. The result is a **3-orders-of-magnitude construct gap** — λ̂ values near 0 vs paper values near 1.

A Python toolkit cannot close this gap. The control surfaces we need to perturb (rate limits, tool latency, mode-switch triggers, rule injection, policy thrash) live inside the Life Agent OS Rust runtime, behind privileged daemon APIs. We need an in-process Rust crate that:

1. Names the perturbations as a typed taxonomy (one enum per level).
2. Hooks the relevant runtime primitive (autonomic tick, arcan provider, autoany rule loop, policy.yaml shield).
3. Subscribes to the resulting Lago event stream and fits `V_k(t) = V_k(0)·exp(−λ̂_recovery·t)` to the recovery curve.
4. Surfaces λ̂_recovery as an OTel span attribute and a metric, so dashboards (Grafana via vigil) can compare it to the paper's λ_i live.

This is `life-perturb`.

## 2. Why an existing toolkit cannot do this

| Candidate | Why it fails |
|-----------|--------------|
| `pumba` / `chaos-mesh` / generic chaos engineering tools | Operate at the OS / k8s layer. Cannot perturb L1 mode flap, L2 rule corruption, L3 policy bit-flip — those live inside the agent process. |
| Python-side perturbation in `microrcs.py` | microRCS doesn't have a runtime — it's a one-shot single-call simulator. No tick loop, no homeostatic state, nothing decays. |
| microgrid simulator | 1-hour timestep. Paper λ values imply sub-second to ~150-second recovery (1/λ_3 ≈ 156s); microgrid cannot resolve. |
| `autonomic-core::rcs_budget::MarginEstimator` | Already exists (life#802) but estimates parameters from observed stationary state. Does not inject perturbations and does not fit recovery curves. |

`life-perturb` is the only candidate that targets the Lyapunov decay rate directly on the live runtime.

## 3. Target Lyapunov Functions per Level

The paper defines V_k abstractly as a Lyapunov function for level k's state space. To measure decay we must commit to **observable** physical proxies. Proposed initial choice — open to revision in v0.5:

### V_0 — Plant (Arcan agent loop)

```
V_0(t) = α · ‖tool_latency(t) − latency_setpoint‖²
       + β · request_drop_rate(t)
       + γ · (1 − provider_health(t))²
```

**Observability:** `arcan-core` already emits per-tick `ToolCall` envelopes with duration; provider health is in `arcan-provider` retry counters. This is the cleanest level.

**Recovery model:** after a `RateLimitStorm` perturbation, V_0 should decay to baseline as the provider recovers and the agent's retry-buffer drains. Paper predicts τ_recovery_0 ≈ 1/1.45 ≈ 0.7s.

### V_1 — Autonomic (Homeostatic state)

```
V_1(t) = w_op  · ‖operational_state(t) − op_target‖²
       + w_cog · context_pressure(t)²
       + w_econ · |economic_mode(t) − Sovereign|       // discrete
```

**Observability:** `autonomic-core::HomeostaticState` is already projected; `MarginEstimator::for_l1` (life#802) consumes it. We piggyback.

**Recovery model:** after a `ModeFlapInduction` (forced rapid transitions Sovereign↔Conserving), V_1 should decay as `HysteresisGate.min_hold_ms` re-stabilises the mode. Paper predicts τ_recovery_1 ≈ 1/0.41 ≈ 2.4s.

### V_2 — Meta-control (autoany / EGRI)

```
V_2(t) = u · ‖rule_set(t) − rule_set_baseline‖_diff
       + v · shadow_eval_veto_rate(t)²
       + w · cross_run_inheritance_lag(t)
```

**Observability:** `autoany-core::loop_engine` emits rule-set deltas; vetoes are countable from shadow-eval logs (already wired in the microRCS L2 path).

**Recovery model:** after a `BadRuleInjection` (force-promote a poisoned rule), V_2 should decay as shadow eval vetoes the rule and the L2 baseline reasserts. Paper predicts τ_recovery_2 ≈ 1/0.07 ≈ 14s.

### V_3 — Governance (policy.yaml + AGENTS.md)

```
V_3(t) = φ · |policy_violation_count(t)|
       + ψ · ‖agents_md_diff(t, baseline)‖
       + χ · governance_drift_score(t)
```

**Observability:** policy violations are already gated in `arcan-aios-adapters` PreToolUse hooks; AGENTS.md drift requires a hash-then-diff on the file. (`agents_md_diff` is the cheapest signal but the slowest — L3 fires on day-scale.)

**Recovery model:** after a `PolicyThrash` perturbation (bit-flip a setpoint in `.control/policy.yaml`), V_3 should decay as the next governance audit (L3 controller) catches and reverts. Paper predicts τ_recovery_3 ≈ 1/0.006 ≈ 156s — the slowest level by 22×.

**v0.1 punts on V_3.** The L3 stability budget λ_3 ≈ 0.006 is so narrow that perturbation experiments must be **extremely** rare (1 per multi-day run, max). We file it as an open question and tackle it in v1.0.

## 4. Perturbation Primitives per Level

The taxonomy from BRO-947, formalised here as an exhaustive enum:

```rust
pub enum PerturbationKind {
    // L0 — agent loop / external plant
    RateLimitStorm     { rps: f64, duration: Duration },
    ToolLatencyJitter  { mean_ms: u32, jitter_ms: u32, duration: Duration },
    RequestDrop        { drop_prob: f64, duration: Duration },
    MemoryCorrupt      { bytes_flipped: u32 },          // L0 boundary
    // L1 — autonomic
    BudgetSkew         { token_delta_pct: f64 },
    ModeFlapInduction  { transitions_per_sec: f64, duration: Duration },
    HysteresisOverride { new_min_hold_ms: u64 },
    // L2 — meta-control
    BadRuleInjection   { rule_text: String, severity: Severity },
    ShadowEvalDisable  { duration: Duration },
    PolicyThrash       { keys: Vec<String>, churn_hz: f64 },
    // L3 — governance (deferred to v1.0)
    GovernanceFlip     { policy_path: String, key: String, new_value: serde_yaml::Value },
}
```

### Per-level injection contract

| Level | Hooks into | Mechanism |
|-------|------------|-----------|
| L0 | `arcan-provider` middleware chain | Tower layer wrapping the provider call; injects latency / drops / 429s before the real request. |
| L1 | `autonomic-controller::HysteresisGate` mutable handle | `min_hold_ms` override + synthetic `HomeostaticEvent` injection through `autonomic-lago` publisher. |
| L2 | `autoany-core::loop_engine::RuleSet::push_unsafe` (new method) | Bypasses the shadow-eval gate to write a controlled bad rule; recorder watches for veto. |
| L3 | `.control/policy.yaml` writer (sandbox copy only — never on production policy file) | Direct file mutation; reverted by control audit on next tick. |

All hooks return a `PerturbationHandle` that records start/end timestamps so the recovery tracker knows the integration window.

## 5. Crate API

The trait surface fits in four modules:

### `perturbation.rs`

```rust
pub enum Level { L0, L1, L2, L3 }
pub enum Severity { Mild, Moderate, Severe }
pub struct PerturbationId(Ulid);

pub enum PerturbationKind { /* see §4 */ }

pub struct Perturbation {
    pub id: PerturbationId,
    pub level: Level,
    pub kind: PerturbationKind,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub duration: std::time::Duration,
}
```

### `injector.rs`

```rust
#[async_trait::async_trait]
pub trait Injector: Send + Sync {
    fn level(&self) -> Level;
    async fn inject(&self, p: &Perturbation) -> Result<PerturbationHandle, PerturbError>;
    async fn revert(&self, h: PerturbationHandle) -> Result<(), PerturbError>;
}

pub struct PerturbationHandle { /* opaque revert token */ }

// Concrete injector stubs (one per level) — implementations in v0.1+
pub struct L0ProviderInjector { /* arcan-provider hook */ }
pub struct L1AutonomicInjector { /* autonomic-controller hook */ }
pub struct L2AutoanyInjector   { /* autoany-core hook */ }
pub struct L3PolicyInjector    { /* policy.yaml sandbox writer */ }
```

### `lyapunov.rs`

```rust
/// Snapshot of (t, V_k(t)) for one observation.
pub struct LyapunovSample { pub t_ms: u64, pub v: f64 }

/// Trait implemented by each level's V_k computer.
pub trait LyapunovFn: Send + Sync {
    fn level(&self) -> Level;
    fn compute(&self, ctx: &SystemSnapshot) -> f64;
}

pub struct V0Plant;        // V_0 from arcan-core ToolCall envelopes
pub struct V1Autonomic;    // V_1 from HomeostaticState (reuses life#802 Estimator)
pub struct V2Autoany;      // V_2 from autoany rule deltas
pub struct V3Governance;   // V_3 from policy + AGENTS.md hashes (v1.0)
```

### `estimator.rs`

```rust
/// Recovery-curve fit:  V_k(t) = V_k(0) · exp(−λ̂ · (t − t_inject)) + ε.
pub struct LambdaEstimator {
    pub level: Level,
    pub samples: Vec<LyapunovSample>,
    pub perturbation: PerturbationId,
}

pub struct RecoveryFit {
    pub lambda_hat: f64,
    pub r_squared: f64,
    pub bootstrap_ci_95: (f64, f64),
    pub n_samples: usize,
    pub fit_window_ms: (u64, u64),
}

impl LambdaEstimator {
    pub fn fit_recovery(&self) -> Result<RecoveryFit, FitError> { /* OLS log-linear */ }
}

/// Top-level orchestrator: inject → record → fit → emit OTel span.
pub struct PerturbationCampaign { /* config + injectors + lyap fns */ }
impl PerturbationCampaign {
    pub async fn run_once(&self, p: Perturbation) -> Result<RecoveryFit, PerturbError>;
}
```

## 6. Integration Points

### 6.1 Daemon hooks

`life-perturb` is **not** a daemon — it's a library consumed by the existing daemons (`autonomicd`, `arcand`, `autoanyd`) under a `--perturb-mode` feature flag. v0.1 plumbs it only into a new `life-perturb-cli` binary:

```bash
life-perturb inject --level 0 --kind rate-limit --rps 100 --duration 60s
life-perturb campaign --config campaigns/l1-mode-flap.toml
life-perturb fit --from-events ./journal/run-2026-05-04.jsonl --level 1
```

The CLI talks to the daemons via their existing UDS / HTTP control plane (no new transport).

### 6.2 Vigil telemetry

Every perturbation emits a span tagged:

```
perturbation.id        = <ulid>
perturbation.level     = "L0..L3"
perturbation.kind      = "RateLimitStorm" | …
perturbation.amplitude = <numeric>
perturbation.duration  = <seconds>
perturbation.lambda_hat= <fit result>
perturbation.r_squared = <goodness>
```

Two new metrics:

```
life.perturbation.lambda_hat{level=…}     histogram (one fit per perturbation)
life.perturbation.recovery_duration_ms    histogram
```

Grafana dashboard JSON ships in v0.5 (under `crates/life-perturb/dashboards/`).

### 6.3 Lago event kinds

Two new event variants behind the existing `EventKind::Custom("perturb.…")` namespace (forward-compatible like autonomic / haima patterns):

```
perturb.injected   { id, level, kind, started_at, duration }
perturb.recovered  { id, lambda_hat, r_squared, ci_95, fit_window_ms }
```

Stored verbatim in `lago-journal`; the recovery tracker subscribes via `lago-aios-eventstore-adapter`.

## 7. Phasing

### v0.1 — single-level smoke (1 week, this PR + 1 follow-up)

- L0 only: `RateLimitStorm` + `ToolLatencyJitter` injectors via `arcan-provider` middleware.
- V_0 computer over real `ToolCall` durations.
- `LambdaEstimator::fit_recovery` OLS implementation.
- CLI: `life-perturb inject` for one-shot perturbations.
- Tests: integration test asserting `λ̂_0 > 0` after a sandboxed rate-limit storm.

**Goal:** a single, reproducible λ̂_0 estimate from a controlled perturbation, even if it's far from 1.45.

#### v0.1 progress (2026-05-05)

| Item | Status |
|------|--------|
| Crate scaffold + spec | ✅ Shipped (PR #1088) |
| `LambdaEstimator::fit_recovery` OLS | ✅ Shipped (PR #1088) |
| `L0ProviderInjector::inject(ToolLatencyJitter)` | ✅ Shipped (this PR, behind `inject-l0` feature) |
| `L0ProviderInjector::revert` | ✅ Shipped (this PR) |
| `L0AutonomicProbe` (V_0 from `HomeostaticState`) | ✅ Shipped (this PR) |
| `L0SimRuntime` closed-loop test harness | ✅ Shipped (this PR) — pure-Rust simulator until live `arcan-provider` Tower-layer wiring lands in v0.5 |
| End-to-end `λ̂_0 > 0` integration test | ✅ Shipped (this PR) — `end_to_end_lambda_hat_zero_is_positive` |
| `RateLimitStorm` injector body | ⏳ Deferred to v0.5 |
| `RequestDrop` injector body | ⏳ Deferred to v0.5 |
| Live `arcan-provider` Tower-layer integration | ⏳ Deferred to v0.5 |
| `life-perturb inject` CLI binary | ⏳ Deferred to v0.5 |

**Deviations from spec.** v0.1 picks `ToolLatencyJitter` as the *single* perturbation kind (rather than landing both `ToolLatencyJitter` and `RateLimitStorm` together) to keep the first PR landable in one sitting; `RateLimitStorm` is the next thing the v0.5 PR adds. The live arcand integration is also deferred — the simulation harness (`L0SimRuntime`) is what gives us the closed-loop `inject → V_0(t) → fit λ̂_0` pipeline today, and the Tower-layer hook lands once the wire-shape is settled.

### v0.5 — closed-loop single-level (3 weeks)

- All three L0 perturbation kinds wired and reverting cleanly.
- L1 added: `BudgetSkew` + `ModeFlapInduction` via `autonomic-controller` hooks.
- `PerturbationCampaign` orchestrator: schedule N perturbations, fit each, aggregate.
- Vigil dashboard JSON for L0 + L1.
- Integration with `autonomicd` and `arcand` under `--perturb-mode`.

**Goal:** ≥ 5 paired (perturb-amplitude → λ̂) data points per level, plotted vs paper analytic value with bootstrap CI.

### v1.0 — full hierarchy (6–8 weeks)

- L2 wired: `BadRuleInjection` + `ShadowEvalDisable` via `autoany-core`.
- L3 wired (sandbox-only): `PolicyThrash` + `GovernanceFlip` against a copy of `policy.yaml`.
- Cross-level orchestrated perturbation chain (e.g. L0 storm → measure cascade through L1).
- Paper figure data: λ̂ vs λ scatter for all 4 levels.
- Production-vs-sandbox split (default sandbox; `--allow-production` requires explicit signed approval).

**Goal:** the headline figure for the RCS validation paper.

## 8. Open Questions

| # | Question | Resolution path |
|---|----------|-----------------|
| 1 | Is OLS log-linear fit sufficient or do we need MLE under heavy-tail noise? | Start OLS; switch to robust regression if R² < 0.7 on first L0 runs. |
| 2 | How to handle natural perturbations during a controlled test? | Co-emit `naturalness.score` per Lyapunov sample; LambdaEstimator excludes windows with score > threshold from the fit. |
| 3 | Sandbox isolation — separate process or in-proc with feature flag? | Default in-proc behind `--perturb-mode` flag + stable revert path. Spawn-separate-process is a v1.0 hardening. |
| 4 | What's the right baseline V_k(0)? Just-before-inject sample, or rolling pre-window mean? | v0.1: just-before-inject. v0.5: 30s rolling mean. |
| 5 | Should L3 perturbations require human approval at injection time? | Yes — gate behind interactive `--confirm-l3` flag. |
| 6 | Do we want Bayesian λ̂ (posterior over λ) instead of bootstrap CI? | Open. v1.0 candidate. |
| 7 | Production deployment: how do we make this safe for the Inirida / Choco / Vaupes pilot sites' microgrid kernels? | Out of scope for this crate — pilot deployments will run only the `--allow-production` subset on pre-approved kernels with operator sign-off. |
| 8 | How do we address the construct-validity question for V_k itself? The proxies in §3 are *one* choice; the paper's V_k is abstract. | We commit to the §3 proxies, document them, and let v1.0 test sensitivity to alternative V_k definitions. |

## 9. Non-goals (explicitly out of scope)

- Real-time perturbation streaming (this is offline analysis).
- Replacing `MarginEstimator` (life#802) — that estimates parameters of the budget formula; we estimate λ from recovery dynamics. They are complementary.
- Causal inference across levels (e.g. "L0 perturbation causes L2 instability"). v1.0 candidate, not v0.x.
- Online adaptation of the controller from λ̂ feedback. Pure measurement here; closed-loop self-tuning is a different research project.

## 10. References

- Paper: `research/rcs/papers/p0-foundations/main.tex`, Theorem 1.
- Canonical params: `research/rcs/data/parameters.toml`.
- Empirical state: `research/rcs/microrcs/THESIS_VALIDATION.md` (read §"Why λ̂ doesn't match paper").
- Linear ticket: BRO-947 (parent BRO-944).
- Existing Rust mirror: `crates/autonomic/autonomic-core/data/rcs-parameters.toml`.
- Existing L1 estimator: `crates/autonomic/autonomic-core/src/rcs_budget.rs::MarginEstimator`.
- F3 observer scaffold: `crates/arcan/arcand/src/rcs_observer.rs` (life#804).
