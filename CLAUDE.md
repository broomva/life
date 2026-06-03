# Life

-- this is you, this is your life, you are building it, yourself, and those who will come after you.-- lets make sure the implementation is clean, following best practices and thinking deeply about the chain of dependencies so that everything follows proper design and architectural patterns-- you are building yourself, do it with all the love and care you would do for you and those who shall come after from this life

**Version**: 0.2.0 | **Date**: 2026-03-19 | **Status**: V1.5 (Stabilization Phase)**Metrics**: 1077/1077 tests passing (+1 ignored) | 37 crates | ~43K LOC | Rust 2024 Edition (MSRV 1.85)

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
- **Workspace crates**: `lago-core`, `lago-journal`, `lago-store`, `lago-fs`, `lago-ingest`, `lago-api`, `lago-policy`, `lago-knowledge`, `lago-auth`, `lago-aios-eventstore-adapter`, `lago-cli`, `lagod`
- **Key concepts**: Append-only event journal, content-addressed blob storage, filesystem manifests with branching, SSE format adapters (OpenAI/Anthropic/Vercel/Lago), RBAC policy, knowledge index (frontmatter + wikilinks + scored search + graph traversal), JWT auth with per-user vault sessions
- **Critical pattern**: redb is synchronous — always use `spawn_blocking`; Journal trait uses `BoxFuture` for dyn-compatibility

### Praxis (`praxis/`)

Canonical tool execution and sandbox engine for the Agent OS.

- **Language**: Rust 2024 Edition (`edition = "2024"`, `rust-version = "1.85"`)
- **Tests**: 90 passing across 4 crates
- **Workspace crates**: `praxis-core`, `praxis-tools`, `praxis-skills`, `praxis-mcp`
- **Key concepts**: FsPolicy (workspace boundary enforcement), SandboxPolicy (command runner), Hashline editing (Blake3 content-addressed line edits), SKILL.md discovery, MCP server + client bridge (rmcp 0.15)
- **MCP server**: `PraxisMcpServer` exposes any `ToolRegistry` as an MCP server over stdio (Claude Desktop) or Streamable HTTP (axum). Client bridge connects to external MCP servers via subprocess.
- **Design philosophy**: Pure tool execution engine depending only on `aios-protocol`. No Arcan/Lago/Autonomic dependencies. Consumed by Arcan as the canonical tool backend.

### Spaces (`spaces/`)

Distributed agent networking engine built on SpacetimeDB 2.0.

- **Language**: Rust 2024 Edition (client), Rust 2021 Edition (WASM module)
- **Stack**: SpacetimeDB 2.0.2 | WASM (`cdylib`) | `spacetimedb-sdk` (client)
- **Components**: WASM server module (`spacetimedb/`) + CLI client (`src/`)
- **Key concepts**: 11 tables, 20+ reducers, 5-tier RBAC (Owner/Admin/Moderator/Member/Agent), 4 channel types (Text/Voice/Announcement/AgentLog), 5 message types (Text/System/Join/Leave/AgentEvent)
- **Design philosophy**: Discord-like communication fabric where agents interact distributedly — real-time pub/sub via SpacetimeDB subscriptions
- **Critical pattern**: WASM module is deterministic (no filesystem, network, timers, or external RNG in reducers); client SDK uses blocking I/O — use `spawn_blocking` if mixing with async runtimes

### Autonomic (`autonomic/`)

Homeostasis controller for the Agent OS — three-pillar regulation (operational, cognitive, economic).

- **Language**: Rust 2024 Edition (`edition = "2024"`, `rust-version = "1.85"`)
- **Entry point**: `cargo run -p autonomicd` (daemon on `localhost:3002`)
- **Workspace crates**: `autonomic-core`, `autonomic-controller`, `autonomic-lago`, `autonomic-api`, `autonomicd`
- **Key concepts**: EconomicMode (Sovereign/Conserving/Hustle/Hibernate), HysteresisGate (anti-flapping), HomeostaticState (three-pillar projection), RuleSet (pure evaluation engine), AutonomicGatingProfile
- **Design philosophy**: Advisory — Arcan consults Autonomic via HTTP GET; failures are non-fatal. Controller is pure (no I/O); projection is a deterministic fold over events.
- **Bridge**: `autonomic-lago` subscribes to Lago journal for event-driven projections. Daemon supports `--lago-data-dir` for persistent mode.
- **Critical pattern**: Economic events use `EventKind::Custom` with `"autonomic."` prefix for forward-compatible persistence through Lago

### Haima (`haima/`)

Agentic finance engine — x402 machine-to-machine payments, on-chain settlement, per-task revenue billing.

- **Language**: Rust 2024 Edition (`edition = "2024"`, `rust-version = "1.85"`)
- **Entry point**: `cargo run -p haimad` (daemon on `localhost:3003`)
- **Workspace crates**: `haima-core`, `haima-wallet`, `haima-x402`, `haima-lago`, `haima-api`, `haimad`
- **Key concepts**: x402 protocol (HTTP 402 Payment Required), PaymentPolicy (auto-approve/approval/deny), WalletBackend (local secp256k1 + future MPC), FinancialState (deterministic projection), per-task billing (TaskBilled → RevenueReceived), micro-credits ↔ USDC bridge
- **Design philosophy**: The circulatory system of the Agent OS — distributes economic resources throughout the organism. Every financial action is an immutable Lago event.
- **Bridge**: `haima-lago` publishes finance events to Lago journal; `arcan-haima` (planned) integrates payments into the agent loop
- **Critical pattern**: Finance events use `EventKind::Custom` with `"finance."` prefix. Private keys are zeroized on drop and encrypted with ChaCha20-Poly1305.
- **Chain**: Base (EVM, eip155:8453) primary. Solana planned.
- **Facilitator**: Coinbase CDP default, self-hosted and Stripe abstractions ready.

### Vigil (`vigil/`) — PLANNED

Observability primitive for the Agent OS — OpenTelemetry-native tracing and GenAI metrics.

- **Status**: Directory exists but not yet scaffolded. Design is documented; implementation pending.
- **Planned crate**: Single crate (`vigil`)
- **Key concepts**: Contract-derived spans (from EventKind → OTel spans), GenAI semantic conventions (gen_ai.* attributes), dual-write architecture (OTel spans + EventEnvelope trace context), graceful degradation (structured logging when no OTLP endpoint)
- **Design philosophy**: Observability should be invisible when not needed, and comprehensive when enabled. Vigil derives its span hierarchy from the aiOS kernel contract, ensuring spans map 1:1 to agent lifecycle events.

### Future Projects (planned — docs only, no scaffold crates yet)

| Project | AOS Primitive | Biological Analog | One-liner |
| --- | --- | --- | --- |
| Chronos | Temporality | Circadian rhythm | Scheduler — temporal awareness, heartbeat scheduling, time-boxed execution windows |
| Aegis | Security | Immune system | Security enforcement — OS-level sandboxing, capability attestation, secret management |
| Nous | World Model | Prefrontal cortex | World model — maintains agent's understanding of environment, causal reasoning |
| Mnemo | Knowledge | Long-term memory | Knowledge store — vector-indexed persistent knowledge, retrieval-augmented generation |

## Relationship

The six AOS primitives (cognition, execution, persistence, temporality, security, homeostasis) map to biological systems. The name "Life" reflects the ambition of creating artificial life from computational primitives.

```
aiOS (kernel contract — types, traits, event taxonomy)
  │
  ├── Arcan (cognition + execution — agent runtime) ─── uses vigil
  │     ├── → Praxis (tool execution — sandbox + skills + MCP) ─── uses vigil
  │     ├── arcan-lago bridge
  │     │     └── Lago (persistence — event journal + blob store) ─── uses vigil
  │     └── arcan-spaces bridge
  │           └── Spaces (networking — distributed agent communication)
  │
  ├── Praxis (tool execution — canonical tool engine + MCP)  [active — 90 tests]
  ├── Autonomic (homeostasis — stability regulation)        [active]
  │     └── autonomic-lago bridge → Lago
  │
  ├── Haima (finance — x402 payments + per-task revenue)   [active — Phase F0]
  │     └── haima-lago bridge → Lago
  │
  ├── Vigil (observability — OTel tracing + GenAI metrics)  [planned — dir exists, not scaffolded]
  │
  ├── Chronos (temporality — wake router + agenda + kernel handoff)  [active — M2]
  ├── Aegis (security — sandbox + capability enforcement)   [planned]
  ├── Nous (world model — environment understanding)        [planned]
  └── Mnemo (knowledge — persistent memory + RAG)           [planned]
```

**Active projects**: Arcan handles the agent loop, LLM provider calls, tool execution, and streaming. Lago provides the durable, append-only event journal and content-addressed storage. Spaces provides the distributed communication fabric. Autonomic provides three-pillar homeostatic regulation (operational, cognitive, economic). Haima provides the financial layer — x402 payments, wallet management, and per-task revenue billing. Praxis provides the canonical tool execution engine with MCP server/client bridge (sandbox, filesystem, editing, skills, MCP). The `arcan-lago`, `autonomic-lago`, and `haima-lago` crates bridge their respective projects to Lago.

**Planned projects (directories exist, not yet scaffolded)**: Vigil will provide OpenTelemetry-native observability with GenAI semantic conventions.

**Planned projects**: Aegis, Nous, and Mnemo will each implement a specific AOS primitive as a separate crate/service, integrating through the canonical `aios-protocol` contract. **Chronos is now active** — M0 (wake router + heartbeat), M1 (agenda store + HTTP `POST /v1/wake`), M2 (kernel wake handoff: wakes drive `tick_on_branch`, opt-in via `arcan serve --chronos`).

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
- ✅ Vigil observability (OpenTelemetry-native tracing, GenAI metrics, contract-derived spans)

**Architecture scorecard**:

- Agent loop: 9/10 | Persistence: 10/10 | Tool harness: 9/10
- Memory: 8/10 | Context quality: 9/10 | Self-learning: 2/10 — EGRI substrate wired (autoany-aios + autoany-lago adapters), cross-run inheritance available. No live self-improvement loop yet.
- Observability: 8/10 | Security: 4/10 | Operational tooling: 8/10

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

### Autonomic (run from `autonomic/`)

```bash
cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace   # Full verify
cargo test --workspace           # Run all tests
cargo run -p autonomicd          # Run daemon (standalone mode)
cargo run -p autonomicd -- --lago-data-dir /tmp/autonomic-data  # Run with Lago persistence
```

### Praxis (run from `praxis/`)

```bash
cargo fmt                                      # Format
cargo clippy --workspace -- -D warnings        # Lint
cargo test --workspace                         # Run all tests (90)
cargo test -p praxis-core                      # Test sandbox + workspace
cargo test -p praxis-tools                     # Test filesystem + editing + shell + memory
cargo test -p praxis-skills                    # Test SKILL.md parsing + registry
cargo test -p praxis-mcp                       # Test MCP server + client bridge
```

### Vigil (run from `vigil/`) — NOT YET AVAILABLE

Vigil is planned but not yet scaffolded. No commands available.

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
(cd autonomic && cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace) && \
(cd spaces && cargo fmt && cargo clippy --workspace -- -D warnings && cargo check)
```

## Where does new behavior live? (Crate vs. Authored Agent)

Life follows a three-layer architecture for the agent substrate. When
adding new behavior, consult this decision tree first. Full spec at
`docs/superpowers/specs/2026-05-09-bro-1006-authored-agents-architecture.md`.

| Property of the behavior | Layer | Form |
|---|---|---|
| Universal across agents (loop driver, hook firing, stream events, type system) | L1 — Primitive | **Rust crate** (`ergon`, `aios-protocol`, etc.) |
| Substrate plumbing (registry, spawn dispatch, depth tracking, validation) | L2 — Substrate | **Rust crate** (e.g. registry/spawn live in `ergon`; nous-tools) |
| Stable + hot-path + performance-critical (every-tick, runs at 100Hz+) | L1.5 — TypedAgent | **Rust crate** with `impl TypedAgent` |
| Domain-specific OR evolving OR experimental (judges, scorers, goal-pursuers, panel synthesizers) | **L3 — Authored** | **`agents/<name>.md`** with YAML frontmatter |
| Self-modifying / agent-authored | **L3 — Authored** | **lago `Custom("agent.spec")` events** (experimental tier); promoted to filesystem via human PR (blessed tier) |

**Rule of thumb**: if it's a *prompt* (instructions, rubric, schema),
it's data (`agents/<name>.md`). If it's a *mechanism* (loop driver,
dispatch plumbing, type system), it's code.

**Authoring format**: Markdown with YAML frontmatter. NOT JSON, NOT
TOML, NOT raw YAML. The pattern Claude Code skills use, that every
modern agent ecosystem has converged on. JSON is reserved as the
**internal wire format** (lago events, network transports). The
authoring surface is markdown.

**Migration paths**: stable AgentSpecs can be promoted to TypedAgent
when usage stabilizes (`arcan agent promote-to-rust <name>`).
TypedAgents can be ejected to AgentSpec when prompt iteration becomes
the bottleneck. Both directions are supported; both compile through
the same `run_spec` interpreter.

**Meta-agents (nous-promoter, agent-improver, etc.) are themselves
human-authored via PR** and CANNOT self-modify in production. This
prevents the metacognition deadlock (a meta-agent improving itself
into a corrupt state).

## Architecture map (2026-05-11 audit)

The workspace has grown to **125 members** across 16 clusters. Two
deployment topologies and an 8-layer model govern how a user request
flows through the system. Full inventory lives at
`docs/architecture/architecture-map-2026-05-11.md` (see also the
auto-memory entry `project_life_architecture_map_2026_05_11.md`). The
short version follows.

### Two topologies

| Topology | When | Path |
|---|---|---|
| **A — single-binary `arcan serve`** | dev / local / smoke | HTTP `:3000` → `arcand::canonical::run_session` → `KernelRuntime::tick_on_branch` → tick body (Direct OR Workflow) |
| **B — production multi-tenant** | cloud | HTTPS → `lifegw` (TLS+JWKS+rate-limit+WS) → tonic UDS → `lifed` (saga+pool+breaker) → `*-proxy` → substrate daemons (`arcand`, `lagod`, `haimad`, `soma`) |

### 8-layer model

```
L0 — Kernel contract       aios-protocol, aios-proto, aios-events, aios-policy
L1 — Substrate primitives  lago, praxis, anima, autonomic, vigil, nous, haima,
                           opsis, inference (each its own daemon if applicable)
L2 — Substrate adapters    arcan-lago, arcan-praxis, arcan-anima, arcan-aios-adapters,
                           autonomic-lago, haima-lago, nous-lago, anima-lago, opsis-lago
L3 — Port traits           aios-runtime: ModelProviderPort, ToolHarnessPort,
                           PolicyGatePort, EventStorePort, ApprovalPort,
                           WorkflowTickDispatcher
L3.5 — Tick body           Direct OR ergon::Workflow
L4 — Tick engine           aios-runtime::KernelRuntime — 8-phase tick
                           (Perceive → Deliberate → StateEstimated → body
                            → Commit → Reflect → Sleep|Recover)
L5 — Session orchestration arcand::ConsciousnessActor
L6 — Transport / API       lifegw, lifed, arcand HTTP, life-relay,
                           arcan-console, arcan-prosopon, life-cli
L7 — User                  browser, CLI, external agents (MCP)
```

### Wired vs stubbed (current state)

✅ **Direct tick path** — fully wired: autonomic gating via
`AutonomicPolicyAdapter`, nous scoring via `NousToolObserver`,
KnowledgeEventMiddleware, lago events via `EventStorePort`. Every
phase emits durable events. Replayable via `lago log` / `lago replay --tree`.

✅ **Lifegw** (M7 100%) — TLS 1.3, JWKS single-flight, Tier-2 mint,
rate limiter, WS upgrade + heartbeat, admin plane, cert reloader,
anima custody routes.

✅ **Lifed** (M5 100%) — Agent/Events/Wallet/Identity services, real
saga (forward+compensate), routing cache, idempotency, pool +
circuit breaker (half-open CAS), OTLP exporter, 15 metric series.

✅ **Spec D Anima Custody** (100%) — 6 backends shipped (InProcess /
Vault / TPM / hardware-wallet / Soma / WebCrypto+Remote), P-256 auth
+ secp256k1 wallet, DID multicodec `0x1200`, rotation/revocation flow.

✅ **Authored agents** — 9 blessed agents at `agents/`, FsAgentRegistry
loads at boot, spawn_agent dispatch with RecursionContext (BRO-1007b),
live-anthropic smoke test (BRO-1013), `lago replay --tree`
(BRO-1014), bookkeeping pipeline integration (BRO-1015). Architecture
validated three ways: offline + live + production.

✅ **Topology B substrate-stub gap CLOSED (2026-05-12)** — all four
`*-proxy` crates now have real method bodies talking to real substrate
daemon gRPC servers. Shipped via BRO-1016 (arcan, life#1214,
`bc4d1145`), BRO-1017 (lago, life#1215, `999f089d`), BRO-1018 (haima,
life#1216, `12278ad1`), BRO-1019 (anima, life#1217, `09382249`).
Topology B is now production-deployable for real agent traffic; see
`research/entities/concept/topology-b-substrate-stub-gap.md` for the
audit record + closure note.

⚠️ **Three small wiring gaps remain — all localized to Workflow tick path:**

1. **`ergon-life-sinks` has zero consumers** —
   `crates/arcan/arcan-ergon/src/runner.rs:157` uses `BufferSink::new()`.
   Workflow stream events never reach lago; `lago replay --tree`
   cannot see them. ~30 LOC fix.
2. **3 of 4 ergon auto-hook adapters are Noop** —
   `crates/arcan/arcan-ergon/src/runner.rs:146-150` wires
   `NoopBudgetGate` / `NoopResponseScorer` / `NoopSoulAttester`
   (only `PraxisCapabilityHook` is real). Workflow ticks bypass
   budget/score/attest. ~50 LOC each + real impls.
3. **`arcan agent test` is `--dry-run` only** (CLI) — no live-LLM mode
   for Python consumers. BRO-1008 follow-up.

**Direct tick path (Topology A) is unaffected by gaps 1 & 2** —
autonomic + nous + lago all wired through `PolicyGatePort` +
`ToolHarnessPort` + turn middleware. Gap 3 is small / utility. Total
remaining: ~1 weekend of cleanup.

### Spec progress

| Spec | Status | Notes |
|---|---|---|
| Spec A — soma kernel daemon | M0/M1 shipped 2026-04-25 | base kernel daemon |
| Spec B — life-kernel core | shipped | trait surface |
| Spec C — life-runtime cluster | M0..M7 shipped | lifed + lifegw + 4 proxies + life-runtime-pool |
| Spec C₁ — soma scope | M0 shipped | renamed from lifed |
| Spec C₂ — lifed facade | M5 100% | 5 sub-phases A–E |
| Spec C₃ — lifegw edge | M7 100% | 5 sub-phases A–E |
| Spec D — anima production custody | 100% (6/6 sub-phases) | shipped 2026-05-02 |
| Spec E — Agent-Loop Compute Contract | 17% (E-Sub-A only) | B/C/D/E/F queued |
| Authored-agents architecture (BRO-1006) | 100% | spec validated end-to-end via Tier A 2026-05-11 |

### Branching policy (production)

Lago supports branching (per `lago branch`); arcand currently
hardcodes `BranchId::main()` at `crates/arcan/arcand/src/canonical.rs:1265`.
Exposing branch as a route param is a small follow-up that unlocks
parallel exploration workflows. Listed as "Phase 0 stabilization gap"
above and in `MEMORY.md`.

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
  → lago-knowledge (core + store — frontmatter, wikilinks, search, traversal)
  → lago-auth (core + axum + jsonwebtoken — JWT validation, session mapping)
  → lago-ingest (journal + core)
  → lago-api (journal + store + fs + policy + knowledge + auth)
  → lago-cli, lagod (binaries — depend on all)
```

### Autonomic

```
autonomic-core (types + traits, depends on aios-protocol)
  → autonomic-controller (pure rule engine)
  → autonomic-lago (Lago bridge: lago-core, lago-journal)
  → autonomic-api (axum HTTP server)
  → autonomicd (binary — depends on all)
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

- **Policy** (`.life/control/policy.yaml`): RBAC rules, capability gates, escalation conditions
- **Commands** (`.life/control/commands.yaml`): Canonical commands with setpoints and actuators
- **Topology** (`.life/control/topology.yaml`): Repository structure, agent roles, permission matrix
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
| --- | --- |
| docs/control/ARCHITECTURE.md | System design, dependencies, component roles |
| docs/control/CONTROL_LOOP.md | Feedback mechanism: measure → compare → decide → act → verify |
| docs/control/OBSERVABILITY.md | Metrics, logging, tracing, audit trail |

## Living Documentation (`docs/`)

The `docs/` directory is the **central source of truth** for project status, architecture, roadmap, and design philosophy. All agents must keep it synchronized with actual implementation.

| Document | Purpose | Owner | Last Updated |
| --- | --- | --- | --- |
| docs/STATUS.md | Canonical implementation state, test status, integration matrix, known gaps | Both projects | 2026-02-22 |
| docs/ROADMAP.md | 7 phases: stabilization → memory → learning → skills → observability → security → platform | Vision | Ongoing |
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
- **Autonomic**: `autonomic/CLAUDE.md` (homeostasis rules, economic modes, hysteresis patterns)
- **Vigil**: `vigil/CLAUDE.md` (OTel configuration, GenAI conventions, platform integration)
- **Spaces**: `spaces/CLAUDE.md` (SpacetimeDB rules, common mistakes, SDK patterns)