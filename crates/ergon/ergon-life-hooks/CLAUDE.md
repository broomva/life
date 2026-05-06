# CLAUDE.md — `ergon-life-hooks` crate

> Instructions for AI agents working in this crate.
> Last updated: 2026-05-06.

## What this crate is

**ergon-life-hooks** is the home of the four "Life-native" auto-registered
hooks the spec (§3.8) calls for: `PraxisCapabilityHook`,
`AutonomicBudgetHook`, `NousScoreHook`, and `AnimaAttestHook`. They're
the bridge between Life's governance substrate (anima / autonomic /
nous / aios capabilities) and ergon's harness loop.

## Spec & tracker

- Spec: `core/life/docs/superpowers/specs/2026-05-05-ergon-v0.1.md` §3.8
- Linear: [BRO-1000](https://linear.app/broomva/issue/BRO-1000)
- Umbrella: [BRO-994](https://linear.app/broomva/issue/BRO-994)

## Core design — adapter-trait pattern

Each hook has a paired **adapter trait** in the same module:

| Hook | Adapter trait |
|---|---|
| `PraxisCapabilityHook` | `CapabilityResolver` |
| `AutonomicBudgetHook`  | `BudgetGate` |
| `NousScoreHook`        | `ResponseScorer` |
| `AnimaAttestHook`      | `SoulAttester` |

The hook takes `Arc<dyn AdapterTrait>` at construction. It uses **only
the trait** in its body — never substrate types directly.

The arcan adapter (BRO-1001) implements each adapter trait against the
real substrate (`PolicySet`, `AutonomicGatingProfile`, `NousEvaluator`,
`AgentSoul`). That keeps substrate dependencies confined to the
adapter — this crate stays substrate-free.

## Why this pattern

- **Zero substrate deps in this crate.** The Cargo.toml depends on
  `ergon` and standard async/serde — that's it. No `anima-core`, no
  `autonomic-core`, no `nous-core`, no `aios-protocol-extension`.
- **Full mockability.** Tests for each hook construct a small mock
  adapter — no substrate ceremony required.
- **Substrate API churn doesn't cascade.** When `anima-core` or
  `autonomic-core` evolves, only the arcan adapter's adapter-trait
  impls move. The hooks, their tests, ergon, and workflow authors are
  all insulated.

## Why a separate crate (not in `ergon`)?

`ergon` is the vendor-neutral harness primitive. These four hooks
encode **Life's** specific governance: which substrates exist, how
they should fire, what failure semantics to use. A future consumer of
`ergon` (e.g., a TS port, a different agent OS) would reasonably want
its own auto-hook set.

Keeping this in its own crate makes that boundary clean.

## Failure semantics — three tiers

Different hooks treat substrate errors differently:

| Hook | Failure mode | Rationale |
|---|---|---|
| `PraxisCapabilityHook` | `Err(reason)` from resolver → `ToolHookOutcome::Deny` | Capability denial is policy; surface to model |
| `AutonomicBudgetHook`  | `Err(reason)` from gate → `InferenceHookOutcome::Deny` | Budget exhaustion is a hard stop |
| `NousScoreHook`        | `Err(reason)` from scorer → `tracing::warn!` + `Continue` | Scoring is observe-only in v0.1; failure is non-fatal |
| `AnimaAttestHook`      | `Err(reason)` from attester → `tracing::warn!` + `Continue` | Attestation infrastructure unavailable shouldn't abort the workflow; observable via telemetry |

If a deployment needs hard-fail-on-attestation, a custom hook can wrap
`SoulAttester` and return `HookOutcome::Deny` on error. The
crate-shipped `AnimaAttestHook` stays tolerant.

## Hook event coverage

All four hooks fire on **exactly one** lifecycle event:

| Hook | Fires on |
|---|---|
| `PraxisCapabilityHook` | `on_pre_tool_use` |
| `AutonomicBudgetHook`  | `on_pre_inference` |
| `NousScoreHook`        | `on_post_inference` |
| `AnimaAttestHook`      | `on_workflow_start` AND `on_workflow_end` |

The other 7 events on each hook default to `Continue`. Each is wired
explicitly (not via the trait's defaults) so the implementer's intent
is visible in the source.

## What this crate does NOT do

- **Construct the hook registry**: that's the arcan adapter's job
  (BRO-1001), since registry construction needs substrate handles.
- **Implement the adapter traits**: again, BRO-1001.
- **Define ordering**: the spec's "auto-hooks fire before user hooks"
  constraint is enforced by the arcan adapter when it appends user
  hooks to the registry it built from these four.

## Useful commands

```bash
cargo check -p ergon-life-hooks
cargo test  -p ergon-life-hooks --all-targets
cargo clippy -p ergon-life-hooks --all-targets -- -D warnings
cargo fmt -p ergon-life-hooks
```

## Don't

- Do not pull in `anima-*` / `autonomic-*` / `nous-*` / `aios-*`
  substrate crate deps. The whole point of the adapter-trait pattern
  is to keep those out. If you need a new piece of substrate behaviour,
  add it as a method on the relevant adapter trait.
- Do not expand a hook to fire on multiple lifecycle events without
  updating the table in the spec deviations section.
- Do not change a hook's failure tier (e.g., make NousScoreHook
  hard-fail) without updating the failure semantics table above and
  the CHANGELOG.
