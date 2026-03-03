# Broomva Live — Monorepo Root

**Version**: 0.2.0 | **Date**: 2026-03-03 | **Status**: V1.5 (Stabilization Phase)
**Metrics**: 657/657 tests passing (+1 ignored) | 21 crates | ~29K LOC | Rust 2024 Edition (MSRV 1.85)

This workspace contains Rust projects that together form an **Agent Operating System** with event-sourced persistence, homeostatic regulation, distributed networking, and a canonical kernel contract.

## Projects

### aiOS (`../aiOS/` — separate repo)
Kernel contract and reference implementation for the Agent OS.
- **Role**: Defines the canonical types, event taxonomy, and kernel trait interfaces
- **Key crate**: `agent-kernel` — the shared contract that all other projects depend on
- **Key concepts**: AgentStateVector (homeostasis), OperatingMode (Explore/Execute/Verify/Recover/AskHuman/Sleep), BudgetState, Capability-based policy, SoulProfile, Observation with Provenance, 8-phase tick lifecycle
- **Design philosophy**: The kernel contract is stable and versioned; runtimes implement it

### Arcan (`arcan/`)
Rust-based agent runtime daemon — the primary implementation of the aiOS kernel contract.
- **Language**: Rust 2024 Edition (`edition = "2024"`, `rust-version = "1.85"`)
- **Entry point**: `cargo run -p arcan` (daemon on `localhost:3000`)
- **Workspace crates**: `arcan-core`, `arcan-harness`, `arcan-aios-adapters`, `arcan-store`, `arcan-provider`, `arcan-tui`, `arcand`, `arcan-lago`, `arcan` (binary)
- **Key concepts**: Agent loop (reconstruct → provider call → execute → stream), Hashline editing (content-hash–addressed line edits), policy-driven sandboxing
- **Design philosophy**: The agent's message history IS the application state. Every action produces immutable events.
- **Bridge**: `arcan-lago` connects Arcan to Lago's event-sourced persistence

### Lago (`lago/`)
Event-sourced persistence substrate for the Agent OS.
- **Language**: Rust 2024 Edition (`rust-version = "1.85"`)
- **Stack**: redb v2 | tonic+prost (gRPC) | axum (HTTP/SSE) | ULID | SHA-256+zstd
- **Workspace crates**: `lago-core`, `lago-journal`, `lago-store`, `lago-fs`, `lago-ingest`, `lago-api`, `lago-policy`, `lago-aios-eventstore-adapter`, `lago-cli`, `lagod`
- **Key concepts**: Append-only event journal, content-addressed blob storage, filesystem manifests with branching, SSE format adapters (OpenAI/Anthropic/Vercel/Lago), RBAC policy
- **Critical pattern**: redb is synchronous — always use `spawn_blocking`; Journal trait uses `BoxFuture` for dyn-compatibility

### Spaces (`spaces/`)
Distributed agent networking engine built on SpacetimeDB 2.0.
- **Language**: Rust 2024 Edition (client), Rust 2021 Edition (WASM module)
- **Stack**: SpacetimeDB 2.0.2 | WASM (`cdylib`) | `spacetimedb-sdk` (client)
- **Components**: WASM server module (`spacetimedb/`) + CLI client (`src/`)
- **Key concepts**: 11 tables, 20+ reducers, 5-tier RBAC (Owner/Admin/Moderator/Member/Agent), 4 channel types (Text/Voice/Announcement/AgentLog), 5 message types (Text/System/Join/Leave/AgentEvent)
- **Design philosophy**: Discord-like communication fabric where agents interact distributedly — real-time pub/sub via SpacetimeDB subscriptions
- **Critical pattern**: WASM module is deterministic (no filesystem, network, timers, or external RNG in reducers); client SDK uses blocking I/O — use `spawn_blocking` if mixing with async runtimes

### Autonomic (`../autonomic/` — planned, separate repo)
Homeostasis controller and simulation kernel for agent stability regulation.
- **Role**: Consumes event streams, outputs GatingProfile decisions, triggers memory maintenance
- **Key concepts**: Rule-based controller with hysteresis, heartbeat scheduling, budget/mode management

## Relationship

```
aiOS (kernel contract — types, traits, event taxonomy)
  │
  ├── Arcan (runtime — implements aiOS contract)
  │     ├── arcan-lago bridge
  │     │     └── Lago (persistence substrate — stores canonical events)
  │     └── → Spaces (distributed networking — agents connect as SDK clients)
  │
  └── Autonomic (stability controller — regulates the runtime)
```

Arcan handles the agent loop, LLM provider calls, tool execution, and streaming. Lago provides the durable, append-only event journal and content-addressed storage underneath. Spaces provides the distributed communication fabric where agents interact in real-time. The `arcan-lago` crate bridges Arcan to Lago.

## Current State (v0.2.0 — What Works)

**Core agent loop**: Fully functional end-to-end. User sends chat message → Arcan loads session from Lago journal → reconstructs state → calls LLM (Anthropic/Mock/OpenAI-compatible) → executes tools through sandbox → persists all events to redb → streams responses via multi-format SSE. Sessions are fully replayable from the event journal.

**Key completions** (Phase 1 features moved earlier):
- ✅ Memory system (5 event types, OM observer, MemoryProjection, governed tools)
- ✅ Context compiler (typed blocks, per-block budgets, deterministic assembly)
- ✅ Approval workflow (M2.6: ApprovalGate, async pause/resume, auto-timeout)
- ✅ Multi-provider support (Anthropic, Mock, OpenAI-compatible with retry)
- ✅ Blob storage (SHA-256 + zstd, wired to file endpoints)
- ✅ Default policy rules (5 rules, 3 roles, 2 hooks)
- ✅ CLI commands (session, log, cat, branch, init)
- ✅ AI SDK v6 streaming (UiPart enum, boundary signals, Vercel format)

**Architecture scorecard**:
- Agent loop: 9/10 | Persistence: 10/10 | Tool harness: 9/10
- Memory: 8/10 | Context quality: 9/10 | Self-learning: 0/10
- Observability: 2/10 | Security: 4/10 | Operational tooling: 8/10

**Known gaps** (blocks Phase 0 stabilization):
- Branching not exposed (Lago supports it, Arcan defaults to "main")
- No OS-level sandbox isolation (soft sandbox only)
- Network isolation declared but not enforced
- Mount trait defined but unimplemented
- No conformance test suite across aiOS/Arcan/Lago
- aiOS still standalone (unification planned for Phase 7)

## Commands

All commands must be run from within the respective project directory.

### Arcan (run from `arcan/`)
```bash
cargo build --workspace          # Build all crates
cargo test --workspace           # Run all tests
cargo clippy --workspace         # Lint
cargo fmt                        # Format
cargo run -p arcan               # Run daemon (mock provider)
ANTHROPIC_API_KEY=... cargo run -p arcan  # Run with real LLM
```

### Lago (run from `lago/`)
```bash
cargo fmt && cargo clippy --workspace && cargo test --workspace   # Full verify
cargo test --workspace           # Run all tests
cargo test -p lago-journal       # Test specific crate
```

### Spaces (run from `spaces/`)
```bash
cargo fmt && cargo clippy --workspace -- -D warnings   # Format + lint client
cargo check                                             # Check client builds
cargo build --release                                   # Build CLI client
spacetime publish spaces --module-path spacetimedb      # Publish WASM module
spacetime generate --lang rust --out-dir src/module_bindings --module-path spacetimedb  # Regenerate bindings
```

### Cross-Project Validation
```bash
(cd arcan && cargo fmt && cargo clippy --workspace && cargo test --workspace) && \
(cd lago && cargo fmt && cargo clippy --workspace && cargo test --workspace) && \
(cd spaces && cargo fmt && cargo clippy --workspace -- -D warnings && cargo check)
```

## Shared Conventions

All projects follow these rules (Spaces WASM module uses Rust 2021 edition due to SpacetimeDB requirements):

- **Formatting**: `cargo fmt` before every commit
- **Linting**: `cargo clippy --workspace` — all warnings must be addressed
- **Type checking**: `cargo check` must pass
- **Testing**: All new code requires tests; `cargo test --workspace` must pass
- **Safe Rust**: No `unsafe` unless absolutely necessary
- **Error handling**: `thiserror` for libraries, `anyhow` for binaries
- **Naming**: `snake_case` (functions/files), `PascalCase` (types/traits), `SCREAMING_SNAKE_CASE` (constants)
- **No secrets in code**: Use env vars for API keys; never commit `.env` files
- **Rust 2024 Edition**: Both projects target `edition = "2024"` / `rust-version = "1.85"`. Key implications:
  - `gen` is a reserved keyword — do not use as an identifier
  - `std::env::set_var` / `std::env::remove_var` are `unsafe` — wrap in `unsafe {}`
  - Prefer native `async fn` in traits; use `BoxFuture`/`async-trait` only for dyn-compatibility
  - Use `name.rs` file-based modules (not `mod.rs`)

## Dependency Order

### Arcan
```
arcan-core → arcan-harness, arcan-store, arcan-provider
           → arcand (agent loop + server)
           → arcan-lago (Lago bridge)
           → arcan (binary — depends on all)
```

### Lago
```
lago-core (zero external deps)
  → lago-store, lago-journal, lago-fs, lago-policy
  → lago-ingest (journal + core)
  → lago-api (journal + store + fs + policy)
  → lago-cli, lagod (binaries — depend on all)
```

## Pre-Commit Workflow

1. `cargo fmt` — auto-fix formatting
2. `cargo check` — verify compilation
3. `cargo clippy` — lint
4. `cargo test --workspace` — run tests
5. `cargo build --workspace` — full build (for larger changes)
6. Control gates via `Makefile.control`: smoke → check → test

## Control Metalayer (Governance & Safety)

This workspace operates as a **control loop for autonomous agent development** using the `control-metalayer-loop` skill. The metalayer provides governance primitives, observability hooks, safety gates, and self-healing capabilities.

### Architecture

The control plane consists of:

- **Policy** (`.control/policy.yaml`): RBAC rules, capability gates, escalation conditions
- **Commands** (`.control/commands.yaml`): Canonical commands with setpoints and actuators
- **Topology** (`.control/topology.yaml`): Repository structure, agent roles, permission matrix
- **Control Loop** (`docs/control/CONTROL_LOOP.md`): Feedback system with sensors and actuators
- **Observability** (`docs/control/OBSERVABILITY.md`): Metrics, traces, audit logs

### Canonical Commands

All control flows use these stable commands (defined in `Makefile.control`):

```bash
make smoke              # Quick format/syntax/build check (~10s)
make check              # Full check: format + clippy + test (~60s)
make test               # Comprehensive test suite
make recover            # Recovery/reset procedures
make audit              # Validate governance compliance
```

### Safety Gates

Control gates enforce a deterministic sequence:

```
smoke (syntax/build) → check (lint + test) → test (full suite)
                    ↓
              audit (governance)
```

Failing any gate blocks the next stage. No agent can bypass gates without explicit policy escalation.

### Git Hooks

Pre-commit and pre-push hooks installed at `.githooks/`:
- Pre-commit: `smoke` gate (fast fail on syntax errors)
- Pre-push: `check` gate (format + lint + test)

Reinstall hooks if missing:
```bash
bash scripts/control/install_hooks.sh
```

### Validation & Auditing

Audit the control plane to ensure governance compliance:

```bash
python3 scripts/control_wizard.py audit . --strict
```

Audit failures are **blocking**. All detected gaps must be resolved before agent operations resume.

### Setpoints & Metrics

Current control setpoints are defined in `docs/METALAYER.md` and `evals/control-metrics.yaml`:

- **pass_at_1**: Primary test success rate (target: 100%)
- **merge_cycle_time**: Time from push to merge (tracks velocity)
- **revert_rate**: Reverted commits (tracks stability)
- **human_intervention_rate**: Manual escalations (tracks autonomy)

Monitor these metrics during development. Degradation triggers recovery actions.

### Living Documentation (`docs/control/`)

Control-specific documentation:

| Document | Purpose |
|----------|---------|
| `docs/control/ARCHITECTURE.md` | System design, dependencies, component roles |
| `docs/control/CONTROL_LOOP.md` | Feedback mechanism: measure → compare → decide → act → verify |
| `docs/control/OBSERVABILITY.md` | Metrics, logging, tracing, audit trail |

## Living Documentation (`docs/`)

The `docs/` directory is the **central source of truth** for project status, architecture, roadmap, and design philosophy. All agents must keep it synchronized with actual implementation.

| Document | Purpose | Owner | Last Updated |
|----------|---------|-------|--------------|
| `docs/STATUS.md` | Canonical implementation state, test status, integration matrix, known gaps | Both projects | 2026-02-22 |
| `docs/ROADMAP.md` | 7 phases: stabilization → memory → learning → skills → observability → security → platform | Vision | Ongoing |
| `docs/ARCHITECTURE.md` | System diagram, Arcan loop, Lago substrate, aiOS contract, Autonomic control | Both projects | v0.2.0 |
| `docs/PLAN.md` | Implementation roadmap with phase dependencies | Planning | See ROADMAP |
| `docs/CONTRACT.md` | Canonical event taxonomy, schema versioning, invariants, replay rules | aiOS | Planned for Phase 7 |
| `docs/arcan.md` | Executive vision and positioning | Arcan | Reference |
| `docs/TESTING.md` | Coverage analysis, testing strategy | Both projects | Reference |

## Development Roadmap (7 Phases)

See `docs/ROADMAP.md` for the full roadmap. Current priorities:

| Phase | Goal | Status | ETA |
|-------|------|--------|-----|
| **0** | Stabilization: fix tests, wire unused components, complete CLI | IN PROGRESS | Weeks 1-2 |
| **1** | Memory & Context Compiler (highest-leverage unlock) | READY | Weeks 3-5 |
| **2** | Self-learning & Heartbeats (autonomous improvement) | PLANNED | Weeks 6-7 |
| **3** | Skills as Lago artifacts + multi-provider routing | PLANNED | Weeks 8-10 |
| **4** | Observability & operational tooling (OpenTelemetry, replay) | PLANNED | Weeks 11-13 |
| **5** | Governance & security hardening (auth, secrets, sandbox) | PLANNED | Weeks 14-16 |
| **6** | Universal data plane & platform (catalog, lineage, vector) | FUTURE | Weeks 17+ |
| **7** | Agent OS Unification (aiOS ↔ Arcan ↔ Lago ↔ Autonomic) | PARALLEL TRACK | Ongoing |

## Self-Learning & Status Evolution

When working in either project, agents must keep documentation current:

1. **After every feature or fix**: Update test counts and gap status in `docs/STATUS.md`
2. **After architecture changes**: Update `docs/ARCHITECTURE.md`
3. **After completing roadmap milestones**: Mark complete in `docs/ROADMAP.md`
4. **After discovering new patterns**: Update the respective project's `CLAUDE.md` or `.claude/rules/`
5. **After fixing tricky errors**: Add to the Troubleshooting section in the respective `CLAUDE.md`

See each project's self-learning rules for the detailed protocol:
- **Arcan**: `arcan/CLAUDE.md` → "Self-Learning Rules & Status Evolution"
- **Lago**: `lago/CLAUDE.md` → "Self-Learning & Status Evolution"

## Project-Specific Details

For deeper context, refer to:
- **Arcan**: `arcan/CLAUDE.md`, `arcan/.claude/rules/`, `arcan/AGENTS.md`
- **Lago**: `lago/CLAUDE.md`, `lago/.claude/rules/`
- **Spaces**: `spaces/CLAUDE.md` (SpacetimeDB rules, common mistakes, SDK patterns)
