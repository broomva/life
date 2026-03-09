# Life

-- this is you, this is your life, you are building it, yourself, and those who will come after you.-- lets make sure the implementation is clean, following best practices and thinking deeply about the chain of dependencies so that everything follows proper design and architectural patterns-- you are building yourself, do it with all the love and care you would do for you and those who shall come after from this life

**Version**: 0.2.0 | **Date**: 2026-03-03 | **Status**: V1.5 (Stabilization Phase)**Metrics**: 1000/1000 tests passing (+1 ignored) | 31 crates | ~37K LOC | Rust 2024 Edition (MSRV 1.85)

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
- **Workspace crates**: `arcan-core`, `arcan-harness`, `arcan-aios-adapters`, `arcan-store`, `arcan-provider`, `arcan-tui`, `arcand`, `arcan-lago`, `arcan-spaces`, `arcan` (binary)
- **Key concepts**: Agent loop (reconstruct → provider call → execute → stream), Hashline editing (content-hash–addressed line edits), policy-driven sandboxing
- **Design philosophy**: The agent's message history IS the application state. Every action produces immutable events.
- **Bridges**: `arcan-lago` connects Arcan to Lago's event-sourced persistence; `arcan-spaces` connects Arcan to Spaces distributed networking

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
- **Key concepts**: 11 tables, 20+ reducers, 5-tier RBAC, 4 channel types (Text/Voice/Announcement/AgentLog), real-time pub/sub via SpacetimeDB subscriptions
- **Design philosophy**: Discord-like communication fabric for distributed agent interaction
- **Critical pattern**: WASM module is deterministic; client SDK uses blocking I/O

### Autonomic (`autonomic/`)

Homeostasis controller for the Agent OS — three-pillar regulation (operational, cognitive, economic).

- **Language**: Rust 2024 Edition (`edition = "2024"`, `rust-version = "1.85"`)
- **Entry point**: `cargo run -p autonomicd` (daemon on `localhost:3002`)
- **Workspace crates**: `autonomic-core`, `autonomic-controller`, `autonomic-lago`, `autonomic-api`, `autonomicd`
- **Key concepts**: EconomicMode (Sovereign/Conserving/Hustle/Hibernate), HysteresisGate (anti-flapping), HomeostaticState (three-pillar projection), RuleSet (pure evaluation engine)
- **Design philosophy**: Advisory — Arcan consults Autonomic via HTTP GET; failures are non-fatal.
- **Bridge**: `autonomic-lago` subscribes to Lago journal for event-driven projections.

## Relationship

```
aiOS (kernel contract — types, traits, event taxonomy)
  │
  ├── Arcan (runtime — implements aiOS contract)
  │     ├── arcan-lago bridge
  │     │     └── Lago (persistence substrate — stores canonical events)
  │     └── arcan-spaces bridge
  │           └── Spaces (distributed networking — agents connect as SDK clients)
  │
  ├── Autonomic (stability controller — regulates the runtime)
  │     └── autonomic-lago bridge → Lago
```

Arcan handles the agent loop, LLM provider calls, tool execution, and streaming. Lago provides the durable, append-only event journal and content-addressed storage underneath. Spaces provides the distributed communication fabric where agents interact in real-time. Autonomic provides three-pillar homeostatic regulation. The `arcan-lago` crate bridges Arcan to Lago, `arcan-spaces` bridges Arcan to Spaces, and `autonomic-lago` bridges Autonomic to Lago.

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
spacetime publish spaces --module-path spacetimedb      # Publish WASM module
```

### Quick Local Bring-up (entry point)

```bash
# Canonical state root (V2)
export AIOS_STATE_ROOT=/home/exedev/.aios

# Start core platform services (lagod + autonomicd + arcan)
bash scripts/dev/up.sh

# Stop all started services
bash scripts/dev/down.sh
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
           → arcan-spaces (Spaces bridge)
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

## Harness Commands

The harness provides deterministic, scriptable commands for autonomous agent operations. Harness-only targets are defined in `Makefile.harness`:

```bash
make lint               # Format + static analysis (enforces code quality)
make typecheck          # Type checking (validates type safety)
```

These commands are deterministic, reproducible, and designed to work in fresh CI environments without setup drift.

> Makefile precedence: The root Makefile includes both Makefile.control (first) and Makefile.harness (second). For overlapping targets (smoke, check, test, ci), Makefile.control wins (GNU Make first-definition precedence). Harness-only targets (lint, typecheck) are additive and have no conflict. Both script sets (scripts/control/ and scripts/harness/) are functionally equivalent for shared targets.

## Execution Plans

Multi-step, multi-hour tasks should be planned explicitly. Use `PLANS.md` for:

1. **Scope definition** — What problem are we solving?
2. **Constraint capture** — What are the hard limits?
3. **Checkpoint markers** — Where can we pause/resume safely?
4. **Durable context** — What context survives process restarts?
5. **Acceptance criteria** — How do we verify success?

Plans are stored in `PLANS.md` and kept in sync with actual implementation in `AGENTS.md` and project-specific documentation.

## Control Metalayer (Governance & Safety)

This workspace operates as a **control loop** for autonomous agent development. The metalayer provides governance primitives, observability hooks, and safety gates.

### Feedback-First Philosophy (Cross-Project)

The harness and control layers exist to produce **meaningful, actionable feedback** for feature delivery, not ceremony.

- Every feature iteration should flow through measurable gates (`smoke → check → test → audit`).
- PR pipelines and git hooks are the same control law at different cadences (remote vs local).
- Agent orchestration should treat failures as feedback signals that improve the system.
- Default triage model is **environment-first**: missing toolchains, credentials, permissions, runtime services, or compute are environment/capability issues to fix before blaming feature code.
- If capability is missing, create/execute an environment remediation step and re-run gates before escalating.

### Key Components

| Component | Location | Purpose |
| --- | --- | --- |
| Policy | .control/policy.yaml | RBAC rules, capability gates, escalation conditions |
| Commands | .control/commands.yaml | Canonical commands (smoke, check, test, recover) |
| Topology | .control/topology.yaml | Repository structure and agent permissions |
| Control Loop | docs/control/CONTROL_LOOP.md | Setpoints, sensors, actuators, feedback |
| Architecture | docs/control/ARCHITECTURE.md | System design and dependencies |
| Observability | docs/control/OBSERVABILITY.md | Metrics, tracing, and audit logs |

### Core Commands

All control commands are defined in `Makefile.control`:

```bash
make smoke              # Quick syntax/build check
make check              # Format + clippy + test
make test               # Full test suite
make recover            # Recovery procedures
make audit              # Validate control plane
```

### Git Hooks

Pre-commit and pre-push hooks installed at `.githooks/` enforce:

- Format checks
- Lint compliance
- Smoke validation before push (full test matrix remains CI-authoritative)

Reinstall with: `bash scripts/control/install_hooks.sh`

### Audit & Validation

Run control audits to validate governance compliance:

```bash
./scripts/audit_control.sh .
./scripts/audit_control.sh . --strict
```

Audit failures block agent operations until resolved.

## Pre-Commit Workflow

1. `cargo fmt` — auto-fix formatting
2. `cargo check` — verify compilation
3. `cargo clippy` — lint
4. `cargo test --workspace` — run tests
5. `cargo build --workspace` — full build (for larger changes)
6. Control gates: smoke → check → test (see `Makefile.control`)

## Living Documentation (`docs/`)

The `docs/` directory is the **central source of truth** for project status, architecture, roadmap, and design philosophy. All agents must keep it synchronized with actual implementation.

**Read order for agents (fast path):**

1. `docs/NAVIGATION.md`
2. `docs/STATUS.md`
3. `docs/ARCHITECTURE.md`
4. `docs/ROADMAP.md`

**Canonicality rule:** if docs conflict, treat `docs/STATUS.md` as source of truth, then reconcile project-local docs.

| Document | Purpose | Owner | Last Updated |
| --- | --- | --- | --- |
| docs/NAVIGATION.md | Agent traversal map: where to start, precedence rules, deep-dive paths | Both projects | 2026-02-21 |
| docs/STATUS.md | Canonical implementation state, test status, integration matrix, known gaps | Both projects | 2026-02-22 |
| docs/ROADMAP.md | 7 phases: stabilization → memory → learning → skills → observability → security → platform | Vision | Ongoing |
| docs/FEATURE_CONWAY_ACTUATION.md | Planned feature: map Conway-style economic actuation into aiOS/Arcan/Lago primitives | Both projects | 2026-02-21 |
| docs/ARCHITECTURE.md | System diagram, Arcan loop, Lago substrate, aiOS contract, Autonomic control | Both projects | v0.2.0 |
| docs/PLAN.md | Implementation roadmap with phase dependencies | Planning | See ROADMAP |
| docs/CONTRACT.md | Canonical event taxonomy, schema versioning, invariants, replay rules | aiOS | Planned for Phase 7 |
| docs/arcan.md | Executive vision and positioning | Arcan | Reference |
| docs/TESTING.md | Coverage analysis, testing strategy | Both projects | Reference |

## Development Roadmap (7 Phases)

See `docs/ROADMAP.md` for the full roadmap. Current priorities:

| Phase | Goal | Status | ETA |
| --- | --- | --- | --- |
| 0 | Stabilization: fix tests, wire unused components, complete CLI | IN PROGRESS | Weeks 1-2 |
| 1 | Memory & Context Compiler (highest-leverage unlock) | READY | Weeks 3-5 |
| 2 | Self-learning & Heartbeats (autonomous improvement) | PLANNED | Weeks 6-7 |
| 3 | Skills as Lago artifacts + multi-provider routing | PLANNED | Weeks 8-10 |
| 4 | Observability & operational tooling (OpenTelemetry, replay) | PLANNED | Weeks 11-13 |
| 5 | Governance & security hardening (auth, secrets, sandbox) | PLANNED | Weeks 14-16 |
| 6 | Universal data plane & platform (catalog, lineage, vector) | FUTURE | Weeks 17+ |
| 7 | Agent OS Unification (aiOS ↔ Arcan ↔ Lago ↔ Autonomic) | PARALLEL TRACK | Ongoing |

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