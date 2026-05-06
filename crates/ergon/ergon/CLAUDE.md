# CLAUDE.md — `ergon` crate

> Instructions for AI agents working in this crate.
> Last updated: 2026-05-06.

## What this crate is

**ergon** is Life's Layer-2 agent-harness primitive. The trait set that lets
a Broomva developer write a `Workflow` in Rust whose deterministic outer body
orchestrates autonomous inner LLM steps, integrated end-to-end with the Life
substrate.

**Layered position** (locked):

```
Layer 4 — Life (Agent OS substrate)
Layer 3 — arcan (runtime daemon)
Layer 2 — ergon (HARNESS — this crate)
Layer 1 — arcan-provider (model wire connectors)
Layer 0 — model
```

## Spec & tracker

- Spec: `core/life/docs/superpowers/specs/2026-05-05-ergon-v0.1.md`
- Linear umbrella: BRO-994 — https://linear.app/broomva/issue/BRO-994

## Status (2026-05-06)

Shipping incrementally per spec §12 work order. **Currently landed** in this
crate:

| File | BRO ticket | State |
|---|---|---|
| `error.rs` | BRO-996 | Done |
| `role.rs` | BRO-996 | Done |
| `stream.rs` (StreamEvent, StreamSink, BufferSink, FanoutSink) | BRO-996 | Done |

**Not yet landed** (follow-up PRs):

| File | BRO ticket | Notes |
|---|---|---|
| `hook.rs` | BRO-997 | Depends on praxis_core::Message, ToolCall, ToolResult — wire types in BRO-998's PR |
| `step.rs` | BRO-998 | Step + StepCtx + InferenceRequest + RuntimeHandle |
| `workflow.rs` | BRO-999 | Workflow + WorkflowExecutor |
| `attestation.rs`, `budget.rs`, `score.rs`, `capability.rs` | BRO-1000 | Auto-registered hooks (anima / autonomic / nous / praxis) |
| `LagoSink`, `VigilSink`, `LifegwSink` | BRO-998 | Default substrate sinks (deferred — pull in lago-journal / life-vigil / mpsc deps) |

## Invariants (DO NOT VIOLATE)

1. **Roles are NEVER persisted in session history.** `Role::merge` produces a
   transient overlay applied only at `ModelRequest` build time. Inserting a
   role into history breaks the call > session > agent precedence rule.

2. **`StreamEvent` variants are append-only after v1.0.** Never remove,
   never reorder semantically. New variants land in any minor version;
   consumers MUST handle `StreamEvent::VendorEvent` for forward compat.

3. **No `unwrap()` / `expect()` / `panic!()` in non-test code.** Workspace
   clippy lints catch these. Use `ErgonError::*` variants.

4. **No emojis in source files.** LLM-friendly diffs.

5. **`async_trait` for now.** When Rust's native `async fn in trait` is
   stable enough for our MSRV (currently 1.93), we'll migrate workspace-wide.

6. **No Life-runtime deps in this crate's foundational layer.** This crate
   currently has zero dependencies on `lago-journal`, `life-vigil`,
   `praxis-*`, `arcan-*`, `anima-*`, `autonomic-*`, or `nous-*`. Those land
   incrementally as their consuming modules ship (per the work-order in
   spec §12).

## License deviation from spec

The spec lists `license = "Apache-2.0"`. This crate uses
`license.workspace = true` (= MIT) for monorepo coherence — life's overall
license is MIT. Documented in `core/life/CHANGELOG.md`.

## Useful commands

```bash
cargo check -p ergon
cargo test  -p ergon --all-targets
cargo clippy -p ergon --all-targets -- -D warnings
cargo fmt -p ergon
```

## Don't

- Do not pull in Life-substrate crate deps speculatively — wire them only
  when the consuming module needs them.
- Do not bypass the role-merge precedence rule.
- Do not introduce `unwrap()` / `expect()` to "fix" a clippy warning.
- Do not rename `StreamEvent` variants after v1.0.
