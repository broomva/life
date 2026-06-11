---
tags:
  - broomva
  - life
  - roadmap
type: operations
status: active
area: system
created: 2026-03-17
---

# Broomva Life: Implementation Status

**Date**: 2026-03-04**Version**: 0.2.0 (canonical baseline)**Rust**: edition 2024, MSRV 1.85+ (Spaces backend: edition 2021)**Tests**: 1049 passing (+5 ignored) across 30 crates + Spaces (32 crates total)

This document is the canonical implementation-state record for `/Users/broomva/broomva.tech/life`.If another status document conflicts with this one, treat this file as source of truth.

## Current State

The baseline unification is active and enforced in production paths:

- `aios-protocol` is the cross-project contract.
- `aios-runtime` is the runtime engine.
- Lago is the persistence backend through canonical port adapters.
- Arcan hosts the canonical runtime and provides integration adapters.
- Public runtime API surface is the canonical session API family.
- 2026-04-08: Reasoning observability Phase 1 landed in shared contracts.
  `aios-protocol` now defines typed `KnowledgeSearched`,
  `KnowledgeRetrieved`, and `KnowledgeEvaluated` events, and `nous-core`
  `EvalContext` now carries optional knowledge metrics for later middleware
  population and evaluator correlation.
- 2026-04-09: Reasoning observability Phase 2 landed on the active runtime path.
  Arcan now emits typed knowledge events from two production seams:
  wake-up knowledge bootstrap (`KnowledgeRetrieved`) and kernel turn
  middleware derived from canonical `ToolCallCompleted` events
  (`KnowledgeSearched`, `KnowledgeRetrieved`, `KnowledgeEvaluated`).
  Autonomic folds the typed knowledge variants directly, and
  `nous-middleware` now populates `EvalContext` with live knowledge
  coverage, freshness, retrieval count, relevance, and query metadata.
- 2026-04-09: Reasoning observability Phase 3 judge substrate landed in
  `nous-judge`. Async `ReasoningCoherence` and `KnowledgeUtilization`
  evaluators now exist, plus `registry_with_reasoning()` for the
  five-evaluator async judge set.
- 2026-04-09: Reasoning observability Phase 4 registry integration is now
  active on the canonical host path. `ToolHarnessObserver` run completion now
  flows through a typed `RunCompletionContext`, `arcand` reconstructs
  assistant output + executed tool summaries + knowledge evidence from the
  canonical event spine, and `NousToolObserver` now executes the shared
  `registry_with_reasoning()` async judge set instead of a hand-built trio.
  The async observer notification path is instrumented under
  `run_observer.notify`, preserving trace lineage for post-run evaluation and
  score publication.
- 2026-04-09: Reasoning observability trace completion is now active across the
  knowledge path. Vigil emits dedicated `knowledge.context_build`,
  `knowledge.search`, and `knowledge.lint` spans; derived `Knowledge*` events
  inherit the source event trace/span IDs; `nous-lago` publishes eval events
  with the current trace context serialized into Lago metadata; and
  `arcan-lago` has an integration test proving wake-up retrieval, search,
  eval, and lint events can be reconstructed as one reasoning trace by
  `trace_id`.
- 2026-04-09: EGRI calibration Phase 5 substrate landed in `lago-knowledge`.
  The crate now exposes a typed `KnowledgeThresholdArtifact` with hard bounds,
  parameterized BM25/search config so threshold mutation affects the live plant,
  and a benchmark schema/runner for Recall@1 and Recall@5 across dev/holdout
  splits. A 50-question seed benchmark file now lives under
  `crates/lago/lago-knowledge/benchmarks/knowledge-benchmark.json`; because the
  entity-page corpus referenced by the approved design was not present in the
  workspace, that file is a bootstrap seed that should be regenerated from the
  canonical entity corpus once mounted.
- 2026-04-10: EGRI calibration Phase 6 proposer substrate is active in
  `lago-knowledge`. `KnowledgeThresholdProposer` now emits deterministic,
  bounded `KnowledgeThresholdProposal`s over the threshold artifact, supports
  single-parameter and correlated mutations, expands after five non-improving
  trials, and filters repeated failed regions plus inherited cross-run
  insights before handing candidates to the future executor/evaluator loop.
- 2026-04-10: EGRI calibration evaluator substrate is active in
  `lago-knowledge`. `KnowledgeQualityEvaluator` now computes the approved
  weighted composite score over dev recall, holdout recall, reasoning
  coherence, knowledge health, token efficiency, and speed; emits
  outcome-compatible metadata; and enforces hard safety plus holdout
  anti-gaming constraints before future trial execution can promote threshold
  candidates.
- 2026-04-10: EGRI calibration trial execution substrate is active in
  `lago-knowledge`. `KnowledgeTrialExecutor` now applies a
  `KnowledgeThresholdArtifact` to the benchmark/search plant, emits
  evaluator-compatible JSON metrics, carries explicit Arcan/Nous runtime signal
  inputs, and returns immutable `KnowledgeQualityOutcome`s for proposer
  feedback and future promotion decisions.
- 2026-04-10: EGRI calibration promotion persistence is active in
  `lago-knowledge`. `promote_to_lago_toml()` validates approved threshold
  artifacts, writes the promoted parameters into the `lago.toml` `[knowledge]`
  section with version/rollback metadata, preserves unrelated TOML sections,
  uses a path-scoped writer lock plus atomic rename, tolerates unversioned
  hand-authored knowledge baselines, and produces an
  `egri.knowledge.promoted` Lago event payload for audit and future Autonomic
  regression monitoring. Local `cargo test -p lago-knowledge` passes with 143
  tests.
- 2026-04-10: EGRI calibration rollback monitoring is active in Autonomic.
  The projection reducer now folds `egri.knowledge.promoted` into typed
  promotion state, counts consecutive post-promotion knowledge-health
  regressions against the promoted `health_threshold`, and folds
  `autonomic.RollbackRequested` acknowledgements. `KnowledgeRegressionRule`
  requests rollback after more than three consecutive regressions by attaching
  a structured advisory event to the gating profile; `autonomic-api` persists
  that advisory event to Lago when a journal is configured, returns the fresh
  published watermark in the gating response, and updates the in-memory
  projection to prevent duplicate rollback requests. Embedded Arcan gating
  acknowledges the same advisory events locally so rollback requests remain
  once-only even without the standalone Autonomic API.
- 2026-04-10: EGRI calibration campaign integration is active in
  `lago-knowledge`. `KnowledgeCalibrationCampaign` now runs the bounded
  proposer → trial runner → evaluator feedback loop, tracks incumbent score
  progression, writes the best qualifying artifact through the existing
  `lago.toml` promotion seam, and exposes a `KnowledgeTrialRunner` trait so
  future Arcan-backed trials and deterministic mock trials share the same
  contract. A cross-crate Autonomic integration test now proves the real
  `egri.knowledge.promoted` payload generated by Lago promotion folds into
  regression counting and emits the expected rollback advisory after sustained
  post-promotion health regression.
- 2026-04-10: Vigil LLM cost envelope integration is active on the Arcan
  provider boundary. `life-vigil` now exposes a richer typed
  `LlmRequestEnvelope` covering request identity, provider routing, token/cost
  economics, reliability, and governance metadata under `vigil.llm.*`
  semantic conventions. `arcan-aios-adapters` builds the envelope for each
  provider call, records it on the GenAI chat span, writes optional JSONL
  artifacts through `VIGIL_JSONL_PATH`, estimates response costs from the
  local pricing snapshot, and returns the serialized call record through
  `ModelCompletion`. `aios-runtime` persists that record as a
  `vigil.llm_call` custom event, so Lago replay can correlate provider
  economics with the same canonical session event spine used for reasoning
  observability and Autonomic regulation.
- 2026-04-10: `memory_graph` v1 is active in Arcan shell. The tool builds a
  derived Lago knowledge index over `.arcan/memory`, resolves starts by exact
  path/name/wikilink target, performs bounded wikilink traversal with cycle
  protection, and returns compact nodes plus `references` edges with
  provenance. The graph remains a derived retrieval layer; no new authoritative
  graph store or mandatory Lago route was introduced.
- 2026-04-10: `memory_graph` hybrid ranking is active. The tool now accepts an
  optional `query`, Arcan can derive semantic ranking hints from the shared
  workspace Lance journal when embeddings are configured, and `arcan-lago`
  scores bounded graph candidates by depth, lexical relevance, optional
  semantic similarity, importance, recency, and edge weight. Missing embeddings
  or vector-search failures degrade to lexical graph ranking or the original
  graph-only BFS fallback; bounds and provenance remain enforced.
- 2026-04-10: `memory_graph` validation and evaluation coverage is active.
  `MemoryGraphResponse` now carries first-class retrieval metrics for returned
  node/edge counts, depth reached, backend, fallback path, and provenance
  preservation. Focused regression fixtures cover causal chains, cycle
  handling, bounds, semantic fallback paths, and missing starts; an Arcan shell
  smoke test proves the tool works through the real `ToolRegistry` +
  `PraxisToolBridge` path.
- 2026-04-12: Nous eval score display and Lago persistence is active on the
  shell path. `/status` now groups Nous scores by evaluation layer
  (safety/execution/action/reasoning/cost) with categorical indicators.
  `/safety` is a new dedicated command showing cumulative safety evaluation
  scores with aggregate stats (min, avg, overall label). Eval scores are
  now persisted as `eval.InlineCompleted` events to the per-session Lago
  journal via fire-and-forget async publish (non-fatal, never blocks REPL).
  `CommandContext.nous_scores` carries full `NousScoreDetail` (name, value,
  layer, label) instead of `(String, f64)` tuples.
- 2026-04-12: Cross-session full-text search is active in lago-knowledge and
  the daemon runtime. `EventSearchIndex` provides BM25-ranked full-text
  search over Lago event payloads (messages, tool results, decisions, errors).
  `EventSearchTool` (`knowledge_search`) is registered as a canonical agent
  tool in the daemon's `ToolRegistry`, enabling agents to search their own
  history across sessions. The index is built lazily from the journal on
  first invocation, caches for repeated queries, and excludes the current
  session (already in conversation context). Internal eval/autonomic/vigil
  events are skipped.

> **2026-04-25 — RENAMED.** The kernel daemon shipped as `lifed` in Phases 0–2 has been renamed `soma` (master spec C, M0). The phase entries below are preserved verbatim as historical record. All ongoing work (Phase 3 + future) uses the `soma` name. Crate path: `crates/life-kernel/lifed/` → `crates/life-kernel/soma/`. Binary: `lifed` → `soma`. CLI `lifectl` folded into the `soma` binary as subcommands. systemd unit: `lifed.service` → `soma.service`. UDS: `/run/lifed/sock` → `/run/life/soma.sock`. See `docs/superpowers/specs/2026-04-25-life-runtime-architecture-spec.md` §L1, §L5.

### lifed Phase 0 — ABI Foundation (2026-04-23 → 2026-04-23)

- ✅ aios-protocol extended: `KernelPort`, `BudgetGatePort`,
  `NetworkIsolationPort`, `HypervisorBackend`, `HypervisorFilesystemExt`
- ✅ 13 new `Kernel*` `EventKind` variants (all additive, `#[non_exhaustive]`)
- ✅ `SandboxTier::MicroVM` variant added
- ✅ `ToolContext` / `ToolResult` gained `wallet`, `cost_hint`,
  `kernel_ctx`, `usage` (all `Option<T>`, fully backward compatible)
- ✅ `SandboxProvider` deprecated-aliased via blanket impl over
  `HypervisorBackend`; `BackendError → SandboxError` conversion wired
- ✅ `arcan-provider-{local,vercel,bubblewrap}` migrated to
  `HypervisorBackend` (+ `HypervisorFilesystemExt` where applicable)
- ✅ `life-kernel-conformance` scaffold crate created (Phase 1 fills it)
- ✅ `scripts/verify_dependencies_lifed.sh` enforces life-kernel-* rules
- Tests: 3184 baseline → 3210 passing (+26 new; 0 regressions)
- 21 commits; 10 Linear tickets (BRO-847..BRO-856) closed in the
  "Phase 0 — ABI Foundation" milestone
- Deprecation warnings from legacy `SandboxProvider` callers in
  `arcan-aios-adapters`, `arcan-lago`, `arcan-praxis`, `praxis-tools`,
  and the `arcan` binary are expected and will be cleaned up in a
  follow-up Phase; the blanket impl keeps all call sites working

### lifed Phase 1 — Kernel Proto + Core Library (2026-04-23 → 2026-04-23)

- ✅ `life-kernel-proto`: ttrpc/tonic wire contract for `KernelService`
  (7 RPCs), generated via prost + tonic-prost-build, bridged to
  `aios-protocol` types (10 round-trip tests)
- ✅ `life-kernel-core`: `KernelEngine` implementing `KernelPort` via
  `BackendRegistry` + `GateChain` + `MeteringWrapper`; pure deterministic
  fold over `EventStorePort` (36 tests including replay determinism +
  proptest state machine)
- ✅ `life-kernel-gate`: `NoOpBudgetGate`, `NoOpNetworkIsolation`,
  `StaticPolicyGate` wrapping `aios-policy::PolicyGatePort` (15 tests)
- ✅ `life-kernel-conformance`: lifecycle + errors + metering + events
  batteries, 15 scenarios total, runs against `arcan-provider-local`
  end-to-end
- ✅ proptest state machine on `GateChain` (3 invariants × 256+ cases each)
- ✅ `cargo fuzz` target on `VmSpec` decoder (manual-run instructions
  documented)
- ✅ Every public API has rustdoc + 5 hot-path doctests
- ✅ Event-replay determinism proven via dedicated integration test
- ✅ `scripts/verify_dependencies_lifed.sh` extended to cover all four
  new crates against the forbidden-dep list
- Tests: 3210 → 3313 passing (+103 new; 0 regressions)
- 20 commits; 10 Linear tickets (BRO-857, BRO-869..877) under the
  "Phase 1 — Kernel Proto + Core Library" milestone

### lifed Phase 2 — Daemon + Observability (2026-04-24)

- 2026-04-24: lifed Phase 2 (Daemon + Observability) shipped. Two new binary
  crates live under `crates/life-kernel/`:
  - `lifed` — privileged daemon hosting the `KernelEngine` from Phase 1, serving
    the tonic `KernelService` over `/run/lifed/sock` (and optional vsock on
    Linux). Lago integration, Vigil spans + `kernel.{vm.lifecycle,dispatch.duration,egress.bytes}`
    metrics, replay-on-restart via `KernelEngine::replay`, graceful shutdown
    with in-flight drain, kill -9 backend resilience, hardened systemd unit.
  - `lifectl` — operator CLI: `create-vm`, `dispatch`, `list-vms` over the same
    tonic contract.
  Tests: lifed 44 (+2 ignored), lifectl 15. Critical-path unblocks Phase 5
  (arcand cutover / MVS ship). PR #1014.
  See `docs/superpowers/plans/2026-04-24-lifed-phase-2-daemon.md`.

- ✅ `lifed` binary: tonic `KernelService` over Unix socket + optional vsock
- ✅ `lifed` binary: Lago `EventStorePort` adapter for `kernel.*` events
- ✅ `lifed` binary: Vigil tracing on every RPC + 3 canonical metrics
- ✅ `lifed` binary: `KernelEngine::replay` on startup (replay-on-restart)
- ✅ `lifed` binary: graceful SIGINT/SIGTERM shutdown with in-flight drain
- ✅ `lifed` binary: kill-9 backend resilience (no silent VM leaks)
- ✅ `lifed` binary: cold-start budget test (bootstrap + bind under 500ms)
- ✅ `lifed` binary: hardened systemd unit with full sandboxing directives
- ✅ `lifectl` CLI: `create-vm`, `dispatch`, `list-vms` over tonic Unix socket
- ✅ End-to-end CI test (`end_to_end_with_stub_backend`) — no Docker required
- ✅ `end_to_end_full` — Docker-gated acceptance test (`#[ignore]`)
- ✅ `example_config.rs` — guards against schema drift in `lifed.example.toml`
- ✅ `listener/mod.rs` → `listener.rs` rename (workspace naming convention)
- ✅ `main()` returns `anyhow::Result<()>` (CLAUDE.md convention for binaries)
- ✅ `unix::serve` visibility tightened to `pub(crate)` with bind→chmod race doc
- Tests: 3313 → 3394+ passing (+81 Phase 2 new; 0 regressions)
- Closes BRO-858, BRO-895, BRO-896, BRO-897, BRO-898, BRO-899, BRO-900,
  BRO-901, BRO-902, BRO-903

### soma rename — kernel daemon (2026-04-25)

- Renamed `lifed` → `soma` per master spec C M0 (Linear BRO-920).
- `lifectl` operator CLI folded into the unified `soma` binary.
- All systemd / config / UDS paths updated; tests green; CI passes.
- Phase 3 (`arcan-provider-cube`) work resumes under `soma` name in M2.
- Spec C₁ — soma scope spec — written at `docs/superpowers/specs/2026-04-25-soma-scope.md` (workspace-level docs branch, PR #23).

### M3 — Proto Consolidation (2026-04-25)

- Canonical proto tree at `core/life/proto/` per master spec §L6.5.
- `aios.v1.*` canonical vocabulary extracted from `kernel.proto`'s in-file
  identifier definitions into `proto/aios/v1/identifiers.proto` (5 message
  types: `VmId`, `VmSnapshotId`, `BackendId`, `SessionId`, `AgentId`).
- `kernel.proto` migrated to `proto/life/kernel/v1/kernel.proto` with
  package rename `broomva.life.kernel.v1` → `life.kernel.v1` (Rust alias
  `broomva_life_kernel_v1` preserved for one minor version).
- New crate `aios-proto` at `crates/aios/aios-proto/` hosts the generated
  Layer-2 proto types. `tonic-prost-build` compiles `proto/aios/v1/*.proto`
  into the `aios::v1` Rust module.
- `aios_protocol::proto_bridge` module ships round-trip `From`/`Into`
  impls for the 5 identifier types (M3.5 extends the bridge to compound
  types like `VmHandle`, `KernelContext`, …).
- buf pipeline live: `proto/buf.yaml` + `proto/buf.gen.yaml`. CI runs
  `buf lint` + `buf breaking` against `origin/main`.
- The 8 v0 service protos (`approvals`, `events`, `model`, `policy`,
  `relay`, `session`, `tools`, `common`) stay at the legacy
  `crates/life-kernel/life-kernel-proto/proto/` location until M3.5/M4.
- Wire compatibility: proto field tags + service shape unchanged.
  Existing `KernelService` clients keep working without recompilation.
- `cargo test --workspace`: baseline 3439 + 5 new bridge round-trip tests
  = 3444 passed / 0 failed / 19 ignored.
- Linear BRO-928. PR #TBD.

### 2026-04-26 — M5 sub-phase A: lifed facade — Agent + Events vs mock substrates

Spec C₂ implementation begins. Sub-phase A ships:

- New `crates/life-runtime/` cluster with `lifed`, four `*-proxy` stubs,
  `life-runtime-proto`, `lifed-conformance` scaffold.
- Public-plane proto: `life.v1.Agent` (full surface), `life.v1.Events`
  (full surface). `life.v1.Wallet` and `life.v1.Identity` skeletons
  (bodies in B13/B14).
- Admin-plane proto skeletons (bodies in C2–C5).
- `lifed daemon` binary boots, binds the configured public UDS, mounts
  `Agent` + `Events` services through a tower-Layer auth middleware.
- Auth: dev signer accepting `Bearer test-token-for-{user_id}`; real
  ES256 + JWKS in B5.
- Routing cache: in-memory shape, by_sid + by_user indices; eviction
  in B8; cold-start replay in D2.
- Saga driver: no-op skeleton; real driver in B6.
- Idempotency-store: in-memory backend; lago-backed in B7.
- Integration tests: `integration_create_session`,
  `integration_send_message`, `integration_spawn_child_stub` all green.
  `Agent.SpawnChild` returns `Status::unimplemented` per Spec C₂ §13.
- New `verify_dependencies_lifed.sh` script + CI lane enforcing
  Spec C₂ §11 dependency rules.
- Tests: 3492 → 3505 (+13 — `integration_create_session`,
  `integration_send_message` (×2), `integration_spawn_child_stub`,
  `example_config` (×2), `routing::cache` unit (×2), `idempotency` unit,
  `life-runtime-proto` codegen smokes (×2), `lifed-conformance` no-op).
- Linear BRO-930 → Done. PR #1047 merged 2026-04-26 (commit `7f91f40`).
  Spec compliance review APPROVED_WITH_CONCERNS (auth-middleware
  lenience comment added in `c85f259` flags B5 follow-up). Code-quality
  review APPROVED_WITH_CONCERNS — "merge as-is" verdict; concerns are
  nice-to-have polish + sub-phase B/C/D/E follow-ups already documented
  in code (5 items, all non-blocking).

### 2026-04-27 — M5 sub-phase B: lifed facade — real substrates + ES256 + Wallet + Identity

Replaces every Sub-phase A mock with real machinery. Sub-phase B ships:

- **4 substrate proxy crates** with real `*Call` traits + builders +
  retry policy + Tier-3 token attachment hook (per-RPC bodies remain
  stubbed; real RPC bodies + connection pools land in D).
- **Real ES256 + JWKS** verifier replaces dev signer at the gate. Invalid
  bearer tokens now early-return `Status::unauthenticated`. The Sub-phase A
  comment-flagged lenience is gone. Dev signer survives for tests via
  `JwksCache::dev_only()` constructor.
- **Real `SagaDriver`** with forward-then-reverse-compensate semantics
  per Spec C₂ §4; compensation failures logged not retried per §4.2.
  Saga state lago persistence deferred to C alongside admin `Saga.Show`.
- **Lago-backed `IdempotencyStore`** with in-memory fallback for tests.
  24h TTL sweeper. `Wallet.Debit` consumes the trait correctly with
  method `"Wallet.Debit"`; replay returns cached receipt.
- **Routing cache eviction**: idle TTL + LRU hard-cap sweeper per
  Spec C₂ §6.3.
- **Full Wallet service**: `GetBalance`, `Statement`, `Debit` (idempotent),
  `Transfer` (Transfer idempotency deferred to D).
- **Full Identity service**: `Me`, `UpdateProfile`, `ListSessions`,
  `RevokeSession` + 30s revoked-sids snapshot publisher. Revoke evicts
  routing entry + inserts to local blocklist.
- **Multi-tab fanout registry** per Spec C₂ §6.4 (per-session pump
  refactor deferred to D6 — currently per-attach pumps).
- **Conformance battery body** (`lifed-conformance`): 5 audiences
  (arcan/lago/haima/anima/soma) substrate-token round-trip verified.
- **`*Dispatch` → `*Call` collapse**: Sub-phase A's bridge-adapter
  `*Dispatch` traits removed; mocks `impl ArcanCall|LagoCall|...`
  directly via the proxy crates.
- **B16 real-substrate bootstrap path**: `run_with_real_substrates`
  exists; mock fallback if any UDS missing (gating behind explicit
  dev-mode flag deferred to D).
- Tests: 3505 → 3516 (+11 — `auth_substrate_token` (4),
  `integration_wallet` (4), `integration_identity` (4),
  `integration_saga_compensate` (4), `conformance_substrate_tokens` (3),
  saga driver unit (2), routing cache unit (+2), `multi_tab_fanout` (1) —
  some net offset from refactored A-baseline tests).
- Linear BRO-931 → Done. PR #1050 merged 2026-04-27 (commit `f1362fc`).
  Spec compliance review APPROVED_WITH_CONCERNS (3 deviations as C/D
  follow-ups — saga lago persistence, ApproveDispatch lock, mock-fallback
  flag). Code-quality review APPROVED_WITH_CONCERNS — "merge as-is"
  verdict; one in-PR fix applied (`689f867` — test rename + CLAUDE.md
  clarification); 6 D-wave follow-ups bundled into BRO-934 (with_token
  wiring, JwksKey PEM, Transfer idempotency, deadline enforcement,
  per-session fanout pump, retry-class proxy errors).
- Sub-phase B follow-up tickets opened: BRO-933 (Sub-phase C — admin
  plane + saga lago persistence + ApproveDispatch lock), BRO-934
  (Sub-phase D — connection pools + circuit breakers + per-RPC bodies).

### 2026-04-27 — M5 sub-phase C: lifed facade — admin plane + saga lago persistence + ApproveDispatch lock

Lights up the operator surface and closes 4 Sub-phase B handoff items.
Sub-phase C ships:

- **Admin-plane UDS** at `/run/life/life-admin.sock` with SO_PEERCRED +
  group-membership-gated authn (no bearer tokens). Permissive fallback
  when `unix_socket_group is None` for dev/tests; production config
  defaults to `Some("life-admin")`.
- **9 admin RPCs functional**:
  - `Runtime.{HealthCheck, SessionsListAll, SessionsForceClose, SessionsSuspend, IdempotencyLookup}`
  - `Saga.{ListInflight, Show}` (+ `ForceCompensate` documented carve-out)
  - `RoutingCache.{Dump, Evict}` (+ `RebuildFromLago` documented carve-out)
- **Saga lago persistence** per Spec C₂ §4.1: `SagaJournal` trait +
  `LagoSagaJournal` impl. `SagaDriver` writes 5 event kinds
  (`saga.started` / `saga.step_forward` / `saga.step_compensated` /
  `saga.completed` / `saga.failed`) to `system/lifed/saga/<saga_id>`.
- **Per-sid `ApproveDispatch` first-responder-wins lock** per Spec C₂
  §6.4: `ApprovalLocks` uses `DashMap<sid, Arc<parking_lot::Mutex<Option<dispatch_id>>>>`.
  16-way concurrent test passes deterministically.
- **`LifedHandles` accessor refactor** on `bootstrap::run_with_mocks`:
  exposes `routing` + `revoked` + `idem` + `saga_registry` +
  `approval_locks` as `Arc`s for tests + admin services.
- **`revoke_session_propagates_to_anima`** widened to assert all three
  effects (anima propagation + blocklist insert + routing cache eviction).
- **Operator CLI**: `lifed admin sessions list`, `lifed admin saga show`,
  `lifed admin routing dump|evict` wired against admin plane.
- Tests: 3516 → 3550 (+34 — admin integration suite covering Runtime ×5,
  Saga ×4, RoutingCache ×3 + saga-lago persistence ×1 + ApproveDispatch
  concurrency ×4 + 5 saga-registry unit + 6 admin-policy unit + 3
  peercred unit + 1 codegen + lifed snapshot_summaries unit).
- Linear BRO-933 → Done. PR #1054 merged 2026-04-27 (commit `7285938`).
  Spec compliance review APPROVED_WITH_CONCERNS (3 carve-out citations
  misattributed to Spec C₂ §16; in-PR fix `77f43de` + `93c7cd9` rewrote
  citations to point at descriptive technical gaps + BRO-934 follow-up).
  Code-quality verdict not separately dispatched — spec reviewer's
  anti-pattern audit + clippy-clean signal sufficient for the admin-only
  surface (no public proxy/transport changes).
- Carve-outs documented in code:
  - `Saga.ForceCompensate` ships as `Status::unimplemented` — needs
    saga-driver re-entrant entrypoint (post-Sub-phase-C follow-up).
  - `RoutingCache.RebuildFromLago` returns 0 entries — needs lago
    `ListNamespaces` RPC, lands in BRO-934 D2 alongside cold-start replay.
  - `lago-proxy::append_event` is a production no-op shim; D2 swaps to
    typed `lago.Append` RPC. Production lifed restarts will lose saga
    history until D2 lands; in-memory `SagaRegistry` keeps `Saga.Show`
    working regardless.

### 2026-04-27 — M7 sub-phase A: lifegw edge gateway — scaffolding + TLS + dev-mode JWT proxy passthrough

Foundation PR for the unprivileged web-facing edge gateway. M7 Sub-phase A
ships:

- **New `crates/life-runtime/lifegw/` cluster**: binary + lib that
  terminates TLS, accepts dev-mode JWTs, mints static-key Tier-2
  capability tokens, and forwards `life.v1.*` unary RPCs to lifed via UDS.
- **`LifegwConfig` schema** with serde defaults + validation;
  `lifegw.example.toml` ships a working dev config. `serde(deny_unknown_fields)`
  + `#[non_exhaustive]` on every public struct.
- **TLS bind via rustls** with self-signed cert test; TLS 1.2/1.3
  default (per `tls12` rustls feature). Production cert hardening lands
  in Sub-phase D.
- **Dev-mode auth**: `Bearer dev-token-for-{user_id}` accepted via
  `dev_signer::verify`; production ES256 + Vercel JWKS lands in
  Sub-phase B as a body swap on the same function signature.
- **Tier-2 mint**: `Tier2Minter::mint` produces an ES256 JWS with
  audience=`lifed`, issuer=`lifegw`, ≤15min lifetime cap (config-validated).
  Static dev keystore for A; KMS-backed swap-in for E.
- **Auth Layer security boundary**: tower middleware reads inbound
  `authorization` (single-value), strips it via `headers_mut().insert`,
  inserts the minted Tier-2 before forwarding upstream. lifed never
  sees the Tier-1 token.
- **`tonic-web` proxy passthrough**: `proxy.rs` forwarders cleanly
  preserve metadata + extensions (so traceparent + the rewritten
  authorization header reach lifed). No business logic in lifegw.
- **`/healthz` endpoint** at axum router level — short-circuits BEFORE
  the auth layer. Returns 200 if upstream lifed UDS reachable, 503
  otherwise.
- **`scripts/verify_dependencies_lifegw.sh` + new CI lane** "Verify
  lifegw dependency rules" enforces Spec C₃ §11: lifegw MUST NOT depend
  on substrate runtime crates, lifed, or the four `*-proxy` crates.
- **systemd unit + socket activation**: `deploy/systemd/lifegw.service`
  drops privileges (`life-runtime-gw` user, no caps,
  `MemoryDenyWriteExecute`, `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX`,
  `SystemCallFilter=@system-service ~@privileged @resources`).
  `lifegw.socket` binds 80+443 with `ListenStream`.
- Tests: 3550 → 3570 (+20 lifegw tests: 3 dev_signer + 5 config + 2
  keystore + 2 tier2 + 3 listener + 2 health + 3 integration including
  `proxy_forwards_create_session`).
- Linear BRO-935 → Done (parent BRO-932). PR #1055 merged 2026-04-27
  (commit `a751ffb`). Code-quality review APPROVED — no critical or
  important issues, "Land this PR." 5 nice-to-have items (missing
  `#[non_exhaustive]` on auth public types, `///` doc gaps, TLS 1.2/1.3
  feature audit, `mod.rs` vs `name.rs` workspace decision, `unauth_response`
  HTTP/200 quirk) deferred to Sub-phase B/C/D as appropriate.

### 2026-04-28 — M7 sub-phase B: lifegw — real Vercel JWKS + ES256 verifier + KMS provider abstraction

Replaces the Sub-phase A dev signer with a production-grade Tier-1
verifier and adds a KMS-backed Tier-2 minting key. Sub-phase B ships:

- **Real Vercel JWKS Tier-1 verifier**: `JwksCache` fetches from
  configured URL, kid-keyed lookup, refetch on miss, 30-min rotation
  grace per Spec C₃ §5.
- **Algorithm-confusion defense**: alg derived from JWKS entry (not JWT
  header); `alg=none` rejected; symmetric algs (HS256/384/512) rejected
  before key lookup; `aud=lifegw` enforced; `iss` enforced; `nbf` + `exp`
  enforced; JWT without `kid` rejected.
- **`Tier2Claims` extension**: `nbf`/`iat`/`jti` added; `#[non_exhaustive]`.
- **`KmsSigner` trait abstraction** with 4 impls:
  - `StaticKeystore` (gated on `dev` feature)
  - `VaultTransit` (default `kms-vault` Cargo feature; HashiCorp Vault
    Transit primary recommendation per Spec C₃ §16 #2)
  - `AwsKms` (feature-gated `kms-aws`, body deferred to E)
  - `GcpKms` (feature-gated `kms-gcp`, body deferred to E)
- **Atomic JWKS publish** to `/run/life/lifegw-jwks.json` via
  `tempfile::NamedTempFile::persist` (POSIX-atomic rename); current +
  previous keys included for rotation grace.
- **TLS 1.3-only listener**: rustls/tokio-rustls `tls12` feature dropped;
  README.md documents the decision.
- **`#[non_exhaustive]`** on 13 lifegw auth public types.
- **`///` doc comments** on `AuthLayer::new` + `AuthService<S>`.
- **Cross-daemon conformance test** (`conformance_published_jwks.rs`):
  lifed-pattern JWKS reader verifies Tier-2 token from JWKS file (no
  shared in-memory state).
- **Integration test** (`integration_jwks_round_trip.rs`): mock Vercel
  JWKS server (axum) → Tier-1 JWT → lifegw verify+mint → forwards to
  lifed mock → success. Plus kid rotation → refetch → still verifies.
- Tests: 3550 → 3595 (+25 lifegw tests: 17 unit + 1 integration with 3
  sub-suites + 4 conformance + bonus dev_signer swap test).
- Linear BRO-936 → Done (parent BRO-932). PR #1057 merged 2026-04-28
  (commit `37b89a3`). Spec compliance review APPROVED_WITH_CONCERNS —
  all 13 CRITICAL security checks PASS; one in-PR fix (`19813dc` —
  README warning about apps/chat JWKS bridge production cutover gate).
  Code-quality review APPROVED — no critical/important findings,
  "Land this PR as-is." 3 deviations bundled into BRO-938 (M7 Sub-phase
  C): `KmsProvider::default()` → `Vault`, `Tier1Claims.tier`
  propagation, route-scope intersection enforcement.

### 2026-04-28 — M5 sub-phase D: lifed facade — pools + breakers + observability + B/C body completions

Production hardening machinery + 9 carry-forward body completions from
Sub-phases B and C reviews. Sub-phase D ships:

- **Per-substrate connection pools** with `ArcSwap<Pool>`: capacities
  match Spec C₂ §7.1 (arcan: 32, lago: 64, haima: 16, anima: 16,
  soma: 8). `PoolGuard` records success/failure on Drop.
- **Hand-rolled circuit breaker** per Spec C₂ §7.2: `FAILURE_THRESHOLD=5`,
  `RATE_THRESHOLD=0.5` over `RATE_WINDOW=30s`, `OPEN_DURATION=10s`.
  All transitions wait-free atomic ops. `failsafe-rs` Cargo feature
  removed (was dead code; cleanup commit `92d6783`).
- **Cold-start replay** scaffolding (`RoutingCache::cold_start` +
  `lago.ListNamespaces` proxy method); typed wire RPC deferred to
  Sub-phase E (lago daemon must ship the RPC first). Lazy-populate
  fallback path matches Spec C₂ §6.3.
- **Bounded mpsc audit**: 9 `mpsc::channel(N)` callsites, 0 unbounded.
- **Observability stack**: `LifedMetrics` registers 15 canonical metric
  series against the global OpenTelemetry meter; `TracePropagationLayer`
  is the outermost middleware on both planes. OTLP exporter wiring
  deferred to Sub-phase E.
- **Slow-consumer policy in fanout**: `STALLED_THRESHOLD=5` consecutive
  Full returns triggers GC. One upstream pump per session via
  `pump_active: AtomicBool` CAS in `FanoutRegistry::try_claim_pump`.
- **`with_token` wired across 4 proxies** via `attach_token` helper.
- **`JwksKey` PEM material** — substrates can verify Tier-3 tokens
  standalone using only the published JWKS file.
- **`Wallet.Transfer` idempotent** via `(user, project, key,
  "Wallet.Transfer")` envelope.
- **`SagaCtx.deadline` enforced** via `tokio::time::timeout_at`;
  `SagaError::Deadline` propagates → `Status::deadline_exceeded`.
- **`RetryClass { Retryable, Permanent }`** symmetric across 4 proxies.
  Pool retries on retryable, fails fast on permanent.
- **`--allow-mock-fallback` flag** (default `false`): production fails
  fast on missing substrate sockets per Spec C₂ §11.4.
- **Chaos test** (`integration_circuit_breaker.rs`): kill-lago →
  breaker opens, arcan unaffected, metric value observable.
- **Backpressure test** (`integration_backpressure.rs`): saturate fanout
  → slow-consumer GC fires, daemon stays up.
- Tests: 3595 → 3640 (+45 — 22 lib + 8 proxy unit + 12 integration +
  3 conformance, including chaos + backpressure + Wallet.Transfer
  idempotency replay + mock-fallback gate).
- Linear BRO-934 → Done. PR #1058 merged 2026-04-28 (commit `614dc84`).
  Spec compliance review APPROVED_WITH_CONCERNS — no critical/missing
  findings; all 12 implementer decisions confirmed correct; 4
  Sub-phase E follow-ups recommended. Code-quality review
  APPROVED_WITH_CONCERNS — 4 IMPORTANT items flagged: 2 in-PR fixes
  applied (`92d6783` removed dead `failsafe-breaker` feature + corrected
  breaker docstring; `55a6fc2` documented deferred pool bracketing in
  wallet/identity/events services); 2 deferred to BRO-937 Sub-phase E
  (half-open trial CAS + RAII PumpGuard).
- Sub-phase D follow-up tickets opened: BRO-937 (Sub-phase E — body
  completions + integration + bake-in), BRO-938 (M7 Sub-phase C — WS
  upgrade + 3 Sub-phase B follow-ups bundled).

### 2026-04-29 — M7 sub-phase C: lifegw — WebSocket upgrade + bidi pump + reconnect + 3 B follow-ups

WebSocket transport for live agent streaming sessions + closes the 3
M7-B follow-ups. Sub-phase C ships:

- **WebSocket upgrade handler** at `/v1/agent/stream` with `?sid=` query
  OR `X-Life-Sid` header (header takes precedence)
- **Bidi pump** connecting client WS frames ↔ lifed
  `Agent.StreamSession` over the same socket
- **Reconnect-by-`last_seq_no`** via `?last_seq_no=` query OR
  `X-Life-Last-Seq-No` header (resume cursor captured in gateway state;
  proto extension to propagate to lifed deferred to a follow-up)
- **Close-code policy** per Spec C₃ §6.5: 1000 normal, 1001 going away,
  1008 policy_violation:token_expired, 1011 internal_error, 4001-4005
  app-level codes (rate-limit, backpressure, ip-blocked,
  lifed-unavailable, sequence-retired)
- **Per-WS bounded mpsc(64)** backpressure with `STALLED_THRESHOLD=5`
  consecutive Full returns triggering 4002 close
- **5 integration + 16 unit + 13 scope module + 4 follow-up tests**
  (38 new total)
- **3 M7-B follow-ups closed:**
  - `KmsProvider::Dev` fail-closed (Option B): rejects unless
    `dev_signer_enabled=true` at signer build time
  - `Tier1Claims.tier` propagates to `Tier2Claims.tier` with
    `DEFAULT_TIER = "free"` back-compat default
  - Route-scope intersection enforced before Tier-2 mint per Spec
    C₃ §5.4: 16-RPC table at `auth/scope.rs`; empty intersection →
    `Status::permission_denied`. D1 fix tightened
    `Agent.StreamSession` to `agent:read` per spec line 468.
- **Bootstrap refactor**: replaced `tonic Server::serve_with_incoming_shutdown`
  with `hyper_util::server::conn::auto::Builder::serve_connection_with_upgrades`
  (tonic 0.14 doesn't enable hyper upgrades). H1/H2 auto-detection
  preserved.
- Tests: 3640 → 3678 (+38).
- Linear BRO-938 → Done (parent BRO-932). PR #1061 merged 2026-04-29
  (commit `b73c098`). Spec compliance APPROVED_WITH_CONCERNS — D1 fix
  applied (`a540641` tightened StreamSession scope per §5.4 line 468);
  D2 deferred to follow-up (proto extension blocked by M5-E proto
  ownership). Code-quality APPROVED_WITH_CONCERNS — 3 IMPORTANT items
  resolved via in-PR doc clarifications (`a235e38`): WS-vs-gRPC scope
  divergence rationale, heartbeat enforcement deferred to Sub-phase D
  with TODO marker, attachment_blob_ref UTF-8-bytes encoding contract
  documented.

### 2026-04-29 — M5 sub-phase E: lifed facade — pool push-down + half-open CAS + RAII PumpGuard + OTLP

Production hardening completion + 8 carry-forward body completions
from Sub-phase D reviews. Sub-phase E ships:

- **New `life-runtime-pool` shared crate** hosting `Pool` (semaphore +
  circuit breaker + ArcSwap-able tonic Channel), `PoolGuard`,
  `CircuitBreaker` (with HalfOpen single-trial CAS), `BreakerState`,
  `SubstrateKind`, `SubstratePools` — dependency-isolation script
  enumerates it explicitly per code-quality review D1 (`1ef05bd`)
- **Pool push-down to all 4 proxy crates** via `Pooled<C>` adapter
  pattern: each `*Proxy` owns `Arc<Pool>` via `with_pool(...)`; per-RPC
  methods bracket internally. Wallet/Identity/Events services no
  longer carry `pools` field. Chaos test now exercises real dispatch
  boundary (the `TestEnv::record_lago_failures` direct-bump stopgap is
  gone).
- **HalfOpen single-trial CAS** via `AtomicBool::compare_exchange` —
  100-concurrent stampede admits exactly 1 guard, 99 short-circuit.
  I1 fix (`ab7d077`) ensures the trial slot is released if
  `Pool::acquire`'s semaphore acquire fails after winning the CAS
  (prevents breaker deadlock on shutdown).
- **RAII `PumpGuard`** for fanout panic-safety: `Drop` impl releases
  `pump_active` slot even if pump task panics. Verified by
  `integration_pump_guard::pump_guard_releases_slot_on_panic`.
- **OTLP exporter wired** behind `cfg.vigil.otlp_endpoint` via
  `life_vigil::init_telemetry`. Logging-only fallback when endpoint
  unset/unreachable. W3C `TraceContextPropagator` installed globally;
  outbound substrate calls propagate `traceparent` per Spec C₂ §9.1.
- **15 metric series incrementing from production code** (Spec C₂ §9.3):
  pool guard owns `dispatch.count` / `dispatch.duration_ms` /
  `semaphore.inflight` / `breaker_state`; routing cache owns
  `session.{created,destroyed,active}_total` / `cache.{size,evictions_total}`
  / `replay_seconds`; saga driver owns `saga.{inflight,completed_total,
  compensation_failed_total}`; fanout owns `slow_stream_total`; new
  `HandlerMetricsLayer` tower middleware owns `handler.duration_ms`.
- **`#[non_exhaustive]`** on RetryClass (×4 proxies) + BreakerState +
  SubstrateKind + SagaEventType. `SubstratePoolsInitial` +
  `SubstrateKind` doc comments on every variant.
- **Lago wire RPC swap (lago.Append + lago.ListNamespaces)** — DEFERRED
  per documented decision: lagod hasn't shipped these typed RPCs yet
  (only `IngestService.Ingest` / `CreateSession` / `GetSession` exist).
  The lago-proxy shim (sha256-keyed `idem_persist` for `append_event`;
  empty-vec fallback for `list_namespaces`) is production-correct.
  `RoutingCache::cold_start` warms on incoming traffic. Tracked under
  BRO-934.
- Tests: 3640 → 3662 (+22 — 15 in `life-runtime-pool` including the
  100-concurrent stampede test and I1 release-slot test, +7 lifed
  including 3 pump-guard panic-safety + 4 substrate-breakers + handler
  metrics tower).
- Linear BRO-937 → Done. PR #1062 merged 2026-04-29 (commit `e2d3a6c`).
  Spec compliance APPROVED_WITH_CONCERNS — all 7 implementer Spec C₂
  decisions verified, 15/15 metric series confirmed, ZERO critical or
  missing findings; 1 in-PR fix (`1ef05bd` adds life-runtime-pool to
  verify script). Code-quality APPROVED_WITH_CONCERNS — 1 IMPORTANT
  fix applied (`ab7d077` HalfOpen trial-slot release on semaphore-closed)
  + 6 nice-to-have items deferred to follow-ups.
- **M5 lifed is now technically 100% production-ready.** Plan tasks
  E1 (`deploy/systemd/lifed.service`) + E3 (E2E smoke harness) remain
  as M5-SHIPPED finalization work — tracked separately. Lago wire RPC
  swap waits on lagod readiness.

### 2026-04-29 — M5 100% SHIPPED — production deploy + bake-in

Closes M5 with the deploy artifacts + smoke harness that the plan's
E1/E3 tasks defined but were deferred from PR #1062's narrower scope:

- **`deploy/systemd/lifed.service`** — hardened systemd unit mirroring
  lifegw's pattern but adapted for lifed's UDS surface. `User=life-runtime`
  with `SupplementaryGroups=life-admin`; drops all capabilities
  (`CapabilityBoundingSet=` empty + `AmbientCapabilities=` empty); full
  `Protect*` and `Restrict*` boilerplate (`ProtectSystem=strict`,
  `ProtectHome`, `PrivateTmp`, `PrivateDevices`, `ProtectKernelTunables`,
  `ProtectKernelModules`, `ProtectKernelLogs`, `ProtectControlGroups`,
  `ProtectClock`, `ProtectHostname`, `ProtectProc=invisible`,
  `ProcSubset=pid`, `LockPersonality`, `MemoryDenyWriteExecute`,
  `RestrictRealtime`, `RestrictSUIDSGID`, `RestrictNamespaces`).
  `RestrictAddressFamilies=AF_UNIX AF_NETLINK` keeps lifed off TCP
  entirely (lifegw owns the public TCP surface per Spec C₂ §11.4).
  `SystemCallFilter=@system-service ~@privileged @resources @reboot`.
  systemd dependency on `lago.service haima.service anima.service
  arcand.service soma.service` so lifed comes up after the substrates.
  `Type=simple` (NOT `Type=notify`) is documented in the unit header
  with the rationale: lifed does not currently emit `sd_notify`, so
  readiness is observable externally by `connect()`-ing to the public
  UDS once the binary forks. Migration to `Type=notify` is queued for
  M6 / Spec C₆.
- **`deploy/systemd/lifed.socket`** — NOT shipped. lifed's listener
  uses `tokio::net::UnixListener::bind` and does not consume `LISTEN_FDS`
  yet. Socket activation is a future enhancement; the rationale is
  recorded in the `lifed.service` header comment.
- **`crates/life-runtime/lifed/tests/e2e_smoke.rs`** — end-to-end smoke
  harness exercising boot → `Agent.CreateSession` (4-step saga) →
  `Agent.SendMessage` (server-stream) → `Agent.StreamSession` →
  `Wallet.GetBalance` → `Wallet.Debit` (idempotent path) →
  `Identity.Me` → graceful drain. Asserts every breaker stays Closed
  under happy path and every substrate's mock-call counter advances.
  A second test (`smoke_test_boot_and_drain_clean`) catches regressions
  that break daemon startup or graceful shutdown without ever
  servicing an RPC. Catches regressions across the whole daemon that
  unit tests miss — boot order, graceful-drain timing, observability
  initialization, pool/breaker bracketing, fanout attachment.
- Tests: 3701 → 3703 (+2 e2e_smoke).
- **Decision (recorded in commit body):** the smoke test asserts
  substrate-mock call counters + breaker states rather than the
  global OpenTelemetry metric registry. OTel `Counter`/`Gauge`/
  `Histogram` instruments aren't directly inspectable from tests;
  asserting that the metric *paths* fired (via the side-effects on
  routing-cache, breakers, and mock-call lists) is a deterministic
  stand-in. A future enhancement could plug an in-memory metric
  reader into `vigil::init_telemetry` for direct value assertion.

**M5 is now production-shipped.** All 5 sub-phases (A-E) complete plus
the E1/E3 deploy + bake-in artifacts. Only the lago wire RPC swap
(BRO-934 follow-up) remains, waiting on lagod to ship typed
`lago.Append` + `lago.ListNamespaces` RPCs.

In-PR review fixes applied before merge (single follow-up commit
`b759b1d7`):

- `deploy/systemd/lifed.service` `RuntimeDirectoryMode=0755` →
  `0750` with explanatory comment, restoring parity with the
  hardening posture of soma + lifegw on shared `/run/life/`.
- Added `PrivateNetwork=yes` to `lifed.service` with explanatory
  comment — lifed is UDS-only and never opens a TCP socket, so a
  network namespace with no devices is the strongest defense-in-depth
  against an exploited dependency that tries to dial an external
  endpoint.
- New `deploy/systemd/README.md` (~190 lines) — operator install
  steps, readiness check via `nc -U`, substrate dependency table,
  `sd_notify` migration plan, socket-activation future enhancement,
  the 15 canonical metric series from Spec C₂ §9.3, hardening
  rationale, and a 7-row symptom→diagnostic→fix runbook. Plan E1
  deliverable.
- New `crates/life-runtime/lifed/lifed.example.toml` (~250 lines) —
  every default in TOML matches in-code default; `OPERATOR ATTENTION`
  comments on the few production-tunable fields
  (`admin_plane.unix_socket_group`, `auth.dev_signer_enabled`,
  `vigil.otlp_endpoint`). Plan E1 deliverable.

PR #1063 merged 2026-04-29 (commit `80a2f9c`). Refs BRO-921 + BRO-937.

### 2026-04-29 — M7 sub-phase D: lifegw — rate limit + admin plane + cert-watch + heartbeat + 9 bundled follow-ups

Closes lifegw operational hardening. Spec C₃ §6 (rate-limiting),
§7 (admin plane), §11.2 (TLS cert reload), §8.2 (heartbeats), §5.4
(JWKS resilience), and §9.4 (admin metrics) are now wired on the
real serving path.

- **D1 — Token-bucket rate limiter** (`services/rate_limit.rs`).
  Per-user (60 QPS, 120 burst) and per-IP (600 QPS, 1200 burst)
  buckets keyed by `(user_id, client_ip)`. Sharded `DashMap`
  with periodic GC every 60s for buckets idle >10m to bound
  memory growth. Tower-layer composition between auth and
  grpc-web so unauthenticated requests get rejected by JWKS first
  (cheaper) and authenticated rate-limit denials carry the
  `(user_id, ip)` tuple in the error event for Vigil. Exempts
  `/life.v1.Identity/Whoami` from per-user metering so a session
  storm cannot lock a user out of identity discovery.
- **D2 — Admin plane** (`admin/{listener,peercred,policy,
  blocklist,service}.rs` + `proto/life/admin/gw/v1/gateway.proto`).
  UDS at `/run/life/lifegw.admin.sock` with mode `0660`
  (`life-runtime:life-admin`). `SO_PEERCRED` reads euid + groups
  on each connection; the connection is rejected if the caller's
  groups do not intersect the configured `admin.allowed_groups`
  set. Three RPCs land: `BlocklistAdd(ip, reason, ttl)`,
  `BlocklistRemove(ip)`, `BlocklistList()`. Blocklist is in-memory
  (sharded `DashMap`) with TTL eviction; persistence across
  restarts is deferred to D-follow-up (admin-plane on lago is the
  long-term plan per Spec C₃ §7.4). Admin plane is feature-flagged
  via `[admin_plane]` config block; daemon refuses to start if
  flag is on but the configured group does not exist on the host.
- **D3 — TLS cert reloader** (`services/cert_watch.rs`). 5-second
  polling on `tls.cert_path` + `tls.key_path` mtime; on change,
  reloads via `rustls::ServerConfig::with_cert_resolver` and
  swaps the `Arc<rustls::server::ResolvesServerCert>` atomically.
  Polling rather than `notify` crate because the deploy target's
  cert-manager writes via `rename(2)` which fires inotify on the
  staging dir, not the published path; mtime polling catches
  every observed cert-rotation pattern reliably. Deferred:
  swapping the `TlsAcceptor` into `serve_connections` (currently
  only the resolver is hot-swappable; the acceptor itself is
  rebuilt-on-rotate but the listener still holds an old
  `TlsAcceptor` in its accept loop — flagged for Sub-phase E).
- **D4 — JWKS single-flight** (`auth/jwks.rs::FlightCoalescer`).
  Hand-rolled `Mutex<Option<Arc<Inner>>>` + `Condvar` cohort
  pattern. First fetcher acquires the inner, every subsequent
  fetcher within the in-flight window blocks on the condvar; on
  completion the leader broadcasts and writes the result into a
  cohort-shared slot (`Arc<Mutex<Option<Result<JwksCache>>>>`)
  so all members observe the same outcome — including failures,
  which propagate uniformly so concurrent requests during a
  Vercel JWKS outage do not stampede.
- **D5 — Heartbeat** (`services/ws.rs`). 30-second ping cadence
  on each upgraded WebSocket; tracks last-pong-rx, terminates
  the session with close-code 1011 (unexpected-condition) if
  three consecutive pings go unanswered. Heartbeats run in a
  dedicated tokio task per session; cancellation-safe via
  `tokio::select!` against the inbound pump.
- **D6 — Reconnect-by-`from_sequence`** (`proto/life/v1/agent.proto`
  `SessionRef.from_sequence` + `services/ws.rs`). Client sends
  `from_sequence: u64` on `Subscribe`; lifegw forwards to lifed
  which streams events `seq > from_sequence`. Bridges client
  reconnect with no event loss across transient network drops.
- **D7 — `Arc<JwksCache>` handle** (`auth/jwks.rs`). The cache
  itself is now `Arc`-wrapped so the cert-watch hot-swap path
  and the request-handling path share the same instance; avoids
  the previous double-buffering hack where the cert-watch
  rebuilt the cache and request-paths held a copy. Memory
  saving is small (one cache); ergonomic gain is large.
- **D8 — `build_signer` fail-closed error class** (`bootstrap.rs`).
  Refactored signer-build error handling to surface exactly
  one error class (`SignerBuildError`) with sub-variants for
  KMS-unreachable / dev-signer-when-not-allowed / key-format.
  Daemon now refuses to start if signer build fails rather than
  falling back to dev signer in production mode.
- **D9 — Dispatcher** (`services/ws.rs`). The bidi pump now
  routes inbound client frames to one of three handler families:
  `RoutedSubscribe` (sets the subscription cursor), `Ack` (debits
  the inbound credit window — paves the way for backpressure
  in Sub-phase E), and `Heartbeat` (records last-pong-rx). All
  unknown frame kinds now close 1003 (unsupported-data) instead
  of 1011, distinguishing protocol violations from server faults
  in the close-code policy.

Per Spec C₃ §13 (admin metrics): `gateway.admin.connection_total`,
`gateway.admin.rejected_total{reason="peercred|group|protocol"}`,
`gateway.blocklist.size`, `gateway.blocklist.match_total` are now
emitted via vigil. Per §9.3 (rate-limit metrics):
`gateway.rate_limit.allowed_total`, `gateway.rate_limit.denied_total
{scope="user|ip"}`, `gateway.rate_limit.bucket_count` round out
the canonical metric set.

Tests: 3703 → 3759 (+56 across rate-limit, admin-plane integration,
heartbeat, JWKS coalescer, cert-watch, dispatcher).

PR #1064 merged 2026-04-29 (commit `571f7efb`). Refs BRO-932 +
BRO-938.

Code-quality review verdict: **APPROVED — merge as-is.** 7 polish
items deferred to a Sub-phase E sweep ticket under BRO-932:

1. `BlocklistEmpty` rename (admin proto error class)
2. `BlocklistList` `Timestamp` simplification (avoid `prost_types`
   round-trip in the hot path)
3. `elapsed.as_millis()` casting cleanup (use `u64::try_from`
   over `as`)
4. IPv6-with-port parser (currently bracket-only; should accept
   `[::1]:port` in admin-plane error reporting)
5. `JwksCache::dump` debug helper for operational triage
6. Supplementary-group lookup robustness (use `getgrouplist(3)`
   over reading `/etc/group` directly)
7. Fail-closed admin policy when group-lookup fails (currently
   logs + denies; should also bump
   `gateway.admin.rejected_total{reason="group_lookup"}`)

Plus the deferred TLS-acceptor swap into `serve_connections` from
D3 (currently the resolver hot-swaps but the listener accept loop
still holds an old `TlsAcceptor`).

### 2026-05-01 — M7-FINAL + D-Sub-A: Spec C M7 closed + Spec D Sub-phase A shipped (Wave 1)

Two parallel streams merged the same evening (2026-05-01) closing
out two milestones: the entire M7 sub-phase E + the M7-D sweep
finally landed (M7 = 100% complete, gateway is production-ready),
and Spec D's first sub-phase ships the AnimaCustody trait plus
P-256 ECDSA migration of the auth keypair (Spec D L4-D6 cutover).

**M7-FINAL (PR #1071, commit `00d97672`)** — closes Spec C M7.

15 atomic items across 6 commits:

KMS production bodies (items 1-6):
- AwsKms body (`auth/kms.rs::AwsKms` impl) — feature `kms-aws`
  via `aws-sdk-kms 1.x`. Sign API call with `MessageType::Raw` +
  `SigningAlgorithmSpec::EcdsaSha256`. DER decoded → raw r||s for
  JWS via shared `der_ecdsa_to_raw_p256` helper. Public key cached
  via `OnceLock` from `GetPublicKey`.
- GcpKms body (`auth/kms.rs::GcpKms` impl) — feature `kms-gcp`
  via `google-cloud-kms 0.6`. Same pattern with `AsymmetricSign`
  against `EC_SIGN_P256_SHA256`.
- VaultTransit `latest_version` pinning — reads
  `body.pointer("/data/latest_version")` instead of hardcoding
  `keys/1`. Without this, key rotation in Vault silently kept
  using the old public key in JWKS.
- VaultTransit token renewal task (`spawn_token_renewal`) —
  `tokio::spawn`'d task with `tokio::time::interval` polling
  `auth/token/renew-self`. Returns `AbortHandle` so `run_daemon`
  can cancel it on graceful shutdown (post-merge B1/I1 fix wired
  this up; was a JoinHandle pre-merge).
- VaultTransit mTLS scaffolding (`with_mtls` accepts
  `Option<VaultMtls>`) — accepts cert/key paths for forward-compat;
  emits `tracing::warn!` and ignores certs in this PR (real mTLS
  deferred to Sub-phase F due to reqwest TLS-feature workspace
  constraint).
- VaultTransit URL_SAFE_NO_PAD `input` encoding — reverted from
  STANDARD back to URL_SAFE_NO_PAD per the post-merge I4 fix
  (Sub-phase B had shipped working with URL_SAFE_NO_PAD; STANDARD
  was changed mid-flight without an integration test backing it).

Chaos test battery (item 6 cont'd) at
`crates/life-runtime/lifegw/tests/chaos_*.rs`:
- `chaos_substrate_uds_drop` — bind-accept-drop scenario verifying
  clean error from `connect_uds` on a dropped socket within 2s.
  Post-merge B4 fix replaced an originally-no-op test with real
  bind+drop+remove+reconnect assertions.
- `chaos_jwks_outage` — 32-thread barrier verifying every cohort
  member surfaces an Err and recovery path covered.
- `chaos_cert_rotation_under_load` — 8 readers + 5 mid-flight
  rotations, verifies ServerConfig instances are distinct
  pre/post and reload counter advances exactly 5×.
- `chaos_kms_unreachable` — exercises fail-closed paths when
  KMS provider returns `LifegwError::Config` at signer-build
  time.

M7-D Sub-phase E sweep (items 7-14):
- proto rename `BlocklistEmpty` → `AdminAck` for naming
  consistency across mutating admin RPCs (BlocklistAdd,
  BlocklistRemove, RateLimitOverride). buf-breaking gated
  via `proto/buf.yaml` `breaking.ignore: [life/admin/gw/v1]`
  for the M7 finalization window.
- `BlocklistList` Timestamp simplification — `prost_types::Timestamp`
  round-trip dropped; production code uses
  `u64::try_from(d.as_millis()).ok().unwrap_or(0)`.
- `as_millis() as u64` cast cleanup — every site replaced with
  `u64::try_from(...).unwrap_or(u64::MAX)` for saturating.
- IPv6-with-port parser (`auth/middleware.rs::parse_ip_or_socket`)
  accepts `[::1]:443`, `1.2.3.4:443`, bare IPv4/IPv6, with 6 unit
  tests.
- `JwksCache::dump` returns `Vec<JwksKeyDump>` with kid + alg + crv
  ONLY (no key material). Wired through admin `JwksDump` RPC.
- `getgrouplist(3)` via nix crate replaces direct `/etc/group`
  reading. Caches per-uid in `Arc<RwLock<HashMap<u32, Vec<u32>>>>`.
  Post-merge I3 fix: also caches the FAILURE path (negative cache)
  to prevent DoS amplification on syscall failure.
- Fail-closed admin policy on group-lookup failure — emits
  `gateway.admin.rejected_total{reason="group_lookup"}` metric
  counter (not just log + deny).
- TLS-acceptor swap into `serve_connections` (D3 deferred item):
  `Arc<rustls::ServerConfig>` swap atomicity verified across the
  accept loop via `arc_swap::ArcSwap` semantics + new
  `AcceptorSource::Reloader` enum. Post-merge B2 fix shares a
  single `Arc<CertReloader>` between SIGHUP, polling watcher,
  and accept loop (was 2 separate instances). Post-merge B3 fix
  wires `CertReloader::spawn_watcher` into production bootstrap.

Spec amendment (item 15):
- `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md` —
  92-line amendment of Spec C₃ §6.5 close-code policy. 10 codes
  documented (1000/1001/1003/1008/1011 + 4001-4005); tonic
  `Code` → `CloseReason` mapping table; reason payload contract;
  4001-4099 vs 4100-4199 split. Post-merge I15 fix reconciled
  drift between the spec table and `services/ws.rs::CloseReason`
  (Unauthenticated/PermissionDenied → 1008, not 1011;
  Cancelled/Aborted → 1000, not 4002; removed nonexistent `Auth`
  variant reference).

Post-merge in-PR review fixes (commit `a73c3784` + `c5fade08` +
`07e9e744`): closed 4 BLOCKING (B1: AWS/GCP wiring + AwsConfig/
GcpConfig in config.rs, B2: shared CertReloader, B3: spawn watcher,
B4: real chaos test) + 4 IMPORTANT (I1: AbortHandle for renewal,
I3: negative-cache supplementary lookup, I4: URL_SAFE_NO_PAD revert,
I15: spec amendment reconciliation) from spec-compliance + code-
quality reviews. RUSTSEC-2026-0098/0099 advisories on rustls-webpki
(transitive via aws-sdk-kms) ignored in `.cargo/audit.toml` (out
of our control until aws-sdk-kms upgrades its TLS chain).

Tests: lifegw 117 lib + 24 integration → 131 lib + 24 integration
+ 4 chaos files (10 tests) + 5 ws-bidi = ~170 lifegw tests passing.
clippy --all-features clean.

PR #1071 merged 2026-05-01 (commit `00d97672`). Refs BRO-932 +
BRO-938. **M7 = 100% complete (A+B+C+D+E all shipped).**

**D-Sub-A (PR #1070, commit `b13ede48`)** — Spec D Sub-phase A.

15 atomic items across 4 commits (2 implementer + 2 review fixes):

Trait abstraction (`crates/anima/anima-identity/src/`):
- `custody.rs::AnimaCustody` trait — 7 methods (user_did,
  auth_pubkey, wallet_address, sign_jws, sign_digest, sign_evm_tx,
  sign_eip712, rotate, export_identity_document, backend_kind).
  `Send + Sync + 'static` bound for `Arc<dyn AnimaCustody>`
  plumbing across tonic/tokio task boundaries.
- `BackendKind` enum — 7 variants (InProcess, Vault, WebCrypto,
  Tpm, Soma, HardwareWallet, Remote) — `#[non_exhaustive]` for
  forward compat with future backends.
- `TxRequest` + `EvmSignature` shapes for the wallet-half signing
  surface.
- `DidRotationEvent` + `DidRotation` (in `anima-core/src/
  identity_document.rs`) for rotation chain.

P-256 cutover (Spec D L4-D6):
- `p256.rs::EcdsaP256Identity` — full API parity with
  `Ed25519Identity` for mechanical swap. `p256` crate with
  `ecdsa, pem, pkcs8` features. SEC1 compressed (33 bytes) for
  public key on the wire. ES256 / SHA-256 signing.
- `did.rs` — both multicodecs supported: `0xed01` Ed25519 (legacy)
  and `0x1200` P-256 (new). `resolve_did_key` dispatches via
  prefix detection and returns `AuthAlg::{Ed25519, P256}`. New
  DIDs default to P-256 (`did:key:zDn…`); Ed25519 stays for
  verifying historical events.
- `seed.rs::derive_p256_key` — HKDF info string `"anima/p256/v1"`
  parallels existing `"anima/ed25519/v1"`.
- `keystore.rs::AnimaKeystore` — production paths
  (`build_identity`, `sign_agent_jwt`) now route through the new
  P-256 identity. Legacy `sign_agent_jwt_ed25519_legacy` retained
  for backwards compat.

InProcessAnima backend (the only D-Sub-A backend):
- `in_process.rs::InProcessAnima` — wraps master seed + derived
  P-256 auth key + secp256k1 wallet key. Spec L4-D7 keeps wallet
  on secp256k1. Constructor `from_seed_arc` returns `Arc<dyn
  AnimaCustody>`; `from_seed` returns concrete type for tests.
- `rotate()` — returns `(DidRotationEvent, Arc<dyn AnimaCustody>)`
  per the post-merge B1 review fix. The original handle stays as
  a snapshot of pre-rotation state; the new handle reflects the
  new key. Wallet half preserved across rotation per L4-D7.
- `rotate_concrete()` — concrete-type variant for callers needing
  `encrypt_seed` on the post-rotation handle.
- `encrypt_seed()` — returns Err on post-rotation handles (post-
  merge I5 review fix) to prevent silent wallet corruption on
  reload.

Call-site refactor (cross-crate):
- `arcan-anima` — new `reconstruct_agent_self_with_custody`
  entry point taking `Arc<dyn AnimaCustody>`; existing
  `reconstruct_agent_self` still works.
- `haima-x402` — new `custody_adapter::CustodyWalletAdapter`
  feature-gated by `custody-adapter`. Wraps the trait's wallet
  half for haima's `sign_transfer_authorization` flow.
- `lago-auth` — new `agent_jwt::detect_alg` + `extract_kid`
  helpers dispatching EdDSA / ES256 verification (full verifier
  body deferred to D-Sub-E).

Event additions (`anima-core/src/event.rs`):
- `anima.identity_rotated { old_did, new_did, rotation_proof_jws,
  rotated_at }` — Spec D L4-D10. The proof JWS is signed by the
  *old* key over the *new* DID.
- `anima.custody_migrated { from_backend: BackendKind, to_backend:
  BackendKind, attestation, migrated_at }` — Spec D L4-D9.
- `anima.identity_revoked { did, reason, revoked_at }`.

Post-merge in-PR review fixes (commit `183c4def`): closed 1 BLOCKING
(B1: rotate() trait contract — returned `(event, Arc<dyn>)` instead
of mutating self) + 2 IMPORTANT (I1: sign_evm_tx SPEC-D-DEVIATION
flagged as RLP stub; I5: encrypt_seed Err on post-rotation) from
spec-compliance + code-quality reviews. Plus a hotfix on main
(commit `695fca2c`) for a pre-existing `lifed::tests::e2e_smoke.rs`
regression — `from_sequence: None` field missing in `SessionRef`
struct literal that M7-D's proto change introduced and M5 SHIPPED's
e2e_smoke didn't pick up.

Tests: anima 111 → 167 (+56 across all anima crates with custody
trait + EcdsaP256Identity + InProcessAnima + new event variants
+ rotation crypto verification). Plus +1 arcan-anima, +5 haima-x402
custody-adapter, +7 lago-auth agent_jwt = ~257 tests across touched
crates.

Coordination items (cross-repo, non-blocking):
- broomva.tech AAP verifier swap (EdDSA → ES256) — separate repo
  needs a follow-up PR. Tracked in `crates/anima/CLAUDE.md`
  under "D-Sub-A coordination items".
- Spaces signed-presence — current life-spaces uses SpacetimeDB
  tables for presence-state, not crypto beacons. When signed
  presence ships, route through `AnimaCustody::sign_digest`.
- lifegw / broomva.tech ES256 verification cohesion — both layers
  now ES256 / P-256; verifiers can share JWKS publish/cache
  plumbing in a future M8 SDK pass.

Spec deviations (`SPEC-D-DEVIATION` comments in `custody.rs`):
- `sign_eip712` only supports EIP-3009 `TransferWithAuthorization`
  shape in D-Sub-A; generic encoder deferred.
- `sign_evm_tx` is a stub — Keccak-256 over JSON canonicalisation,
  NOT EIP-155 RLP. Not on the production hot path (haima-x402
  uses `sign_eip712`); proper RLP defer to D-Sub-B.
- `rotate()` returns event + new handle but doesn't write to Lago
  (caller's responsibility — matches lifegw `KmsSigner::publish_jwks`).

PR #1070 merged 2026-05-01 (commit `b13ede48`). **D-Sub-A complete
— `AnimaCustody` trait + `InProcessAnima` shipped.** Refs Spec D
L4-D5..D10. Next: D-Sub-B (VaultTransitAnima) blocks nothing on the
trait; can land in parallel with M8.

**Wave 1 summary:**
- M7 = 100% complete. Gateway is production-ready.
- Spec D = 17% complete (1/6 sub-phases). Trait shape locked,
  P-256 cutover done, six future backends now implementable.
- Tests delta: ~3759 + ~57 (D-Sub-A) + ~26 (M7-FINAL) = ~3842
  workspace tests passing.
- Critical-path next: M8 SDK (was M7-blocked, now unblocked).
- D-Sub-C is M9-blocking (browser path).

### 2026-05-01 — Wave 2A: D-Sub-B (Vault custody) + M8 SDK shipped

Two parallel streams merged 2026-05-01 on the foundation laid by Wave 1
(M7-FINAL + D-Sub-A). Both required substantive in-PR review fixes
before merge.

**D-Sub-B — `VaultTransitAnima` (PR #1073, commit `bf409af5`):**
Spec D's first production-grade `AnimaCustody` backend — HashiCorp
Vault Transit per-user namespaces (`anima-{user_id}-auth-v{n}` +
`anima-{user_id}-wallet-v{n}`). Auth half: P-256 (ES256, JWS via
`marshaling_algorithm: "jws"`); wallet half: secp256k1 with EIP-1559
RLP digest computed via the new shared `crate::rlp` module + Vault
`prehashed: true` signing. New crate-level `rlp.rs` (471 LOC, 18 unit
tests) closes D-Sub-A's `sign_evm_tx` SPEC-D-DEVIATION — both
`InProcessAnima` and `VaultTransitAnima` now produce identical
signed-over digests for the same `TxRequest`.

Highlights:
- Per-user namespace pattern with strict `validate_user_id` (B1
  review fix) — alphanumeric + `_-` only, 1..=64 chars; rejects
  path-traversal vectors like `"alice/../admin"`.
- Cross-backend digest equivalence test (B2 review fix) — locks the
  L4-D7 invariant that whichever backend signs, the signed-over
  bytes are byte-for-byte identical.
- `parse_data_hex` strict-prefix enforcement (I2 review fix) —
  rejects ambiguous unprefixed hex strings to prevent silent calldata
  corruption.
- `compute_v_byte` ecrecover loop (Vault doesn't return `v` for
  secp256k1 sigs — try both 27 + 28 + match against cached
  uncompressed wallet pubkey).
- `key_version: latest-1` rotation-proof signing — Vault's
  `transit/sign` defaults to latest, so post-rotate the proof needs
  explicit pin to the OLD version. Solves the L4-D10 rotation-proof
  contract for Vault.
- `kms-vault` feature gate (default off; mirrors lifegw's pattern).
- mTLS forward-compat scaffolding (warning + ignored at runtime;
  full plumbing deferred to Sub-phase F when reqwest TLS-feature
  workspace bump lands).
- `spawn_token_renewal` returns `AbortHandle` (matches lifegw's I1
  fix from M7-FINAL).
- 5 wiremock-backed integration tests + 1 `#[ignore]`-gated live
  Vault dev-server test + 18 RLP unit tests + 6 vault-internal unit
  tests + 3 user_id validation tests + 1 cross-backend digest test.
  anima-identity 113 → 131 (+18).

Deferred follow-ups (per code-quality reviewer triage):
- Live `vault server -dev` USDC end-to-end on Base fork — Vault
  v1.15 doesn't support secp256k1 transit keys natively; achievable
  when Vault patches land or HSM sidecar lands.
- Generic EIP-712 encoder — D-Sub-A's EIP-3009-only limitation
  retained; defer to D-Sub-E (SomaCustody) when typed-data signing
  of arbitrary payloads becomes a real workflow.
- Full mTLS feature plumbing (workspace reqwest pin).
- Rotation race-condition documentation (multi-writer concurrent
  rotates; documented as single-writer assumption for now).

**M8 SDK — `@broomva/life-sdk` v0.1.0-pre (PR #1072, commit
`e6106cdb`):** the public-facing TypeScript SDK at
`sdks/life-sdk-ts/`. Hand-curated proto-type wrappers (no
`@bufbuild/protobuf` runtime — kept the bundle slim) + Connect
protocol JSON over fetch + WS state machine for `Agent.StreamSession`
with reconnect-by-`from_sequence` + close-code → typed-error mapping
matching the Spec C₃ §6.5 amendment. 50 vitest tests across 6 files;
4 user-facing services (Agent / Events / Wallet / Identity); 9 typed
error classes; 2 examples (quickstart + streaming).

Highlights:
- 4 services + 13+ RPCs map 1-to-1 with `proto/life/v1/*`.
- WS reconnect logic with permanent-vs-transient close-code
  discrimination (post-merge I-3 fix correctly handles
  close-before-open on retry).
- Stream reader leak fix (post-merge I-2): `try/finally` +
  `releaseLock()` + outer `controller.abort()` so consumer
  early-break frees the HTTP connection.
- Proto type honesty (post-merge I-1): proto3-JSON canonical
  encoding for `int64`/`uint64` is `string` and `bytes` is base64
  string. Pre-fix the SDK declared these as `bigint` /
  `Uint8Array` but the JSON reviver was a no-op so consumers
  calling `bal.micros - 1n` would have crashed.
- New `src/codec.ts` exports `microsToBigInt` / `bigIntToMicros` /
  `sequenceToBigInt` / `bigIntToSequence` / `bytesFromBase64` /
  `bytesToBase64` for callers that need bigint arithmetic or raw
  bytes.
- Backpressure-error coverage test (post-merge I-4): added a
  companion test to the auto-reconnect test that disables
  reconnect and asserts `BackpressureError` reaches `onError`.
- README invocation fix (post-merge I-6): replaced the
  `pnpm --filter` command (which fails without a workspace
  config) with a `cd sdks/life-sdk-ts && pnpm run example:quickstart`
  flow.
- Wallet idempotency doc (post-merge spec I2): clarified
  `TransferReq.memo` is the M5 Sub-phase D idempotency key for
  `Wallet.Transfer` per Spec C₂ §3.3 — the proto schema doesn't
  carry a dedicated idempotency field, so memo serves the role.

**Marked v0.1.0-pre with `KNOWN_LIMITATIONS.md` documenting two
deferred wire-protocol gaps** (B1 + B2 from spec-compliance review):

- **B1 — Connect-vs-grpc-web wire mismatch:** the SDK speaks
  Connect-protocol JSON (`application/json`,
  `application/connect+json`) but lifegw mounts
  `tonic_web::GrpcWebLayer` which only accepts
  `application/grpc-web*`. v0.1.0-pre will NOT successfully complete
  real calls against a production lifegw. Tests pass against the
  in-tree FakeGateway because it accepts both. Resolution path A:
  switch SDK transport to grpc-web binary framing with
  `@bufbuild/protobuf` codec (Connect-ES already supports grpc-web
  mode; only Transport's internal framing changes). Tracked as
  v0.2.0 follow-up "M8.1".
- **B2 — Browser WS auth via subprotocol unsupported by gateway:**
  browsers can't set `Authorization` on WS upgrades. SDK forwards
  bearer via `Sec-WebSocket-Protocol: bearer.<token>`; lifegw's
  `parse_upgrade_request` reads only `Authorization` header. Browser
  WS path will fail Tier-1 auth on production deployments.
  Resolution path A: add subprotocol-bearer parsing to lifegw's
  `parse_upgrade_request` + `AuthLayer` (gateway-side change).
  Tracked as follow-up "M8.2".

The SDK's structural foundation (services, errors, WS state machine,
proto types, codec helpers, 50 tests) IS solid and reusable — only
the wire-codec layer needs replacement in v0.2.0.

**Wave 2A summary:**
- Spec D = 33% complete (2/6 sub-phases shipped; D-Sub-A
  `InProcessAnima` + D-Sub-B `VaultTransitAnima`).
- M8 SDK = scaffolding shipped as v0.1.0-pre; v0.2.0 transport
  rework (M8.1) + lifegw browser-auth subprotocol support (M8.2)
  are the path-to-production tickets.
- Tests delta: ~3842 + ~18 (D-Sub-B) + 50 (M8 SDK vitest) =
  ~3860 Rust workspace + 50 TypeScript SDK.
- Critical-path next: D-Sub-C (WebCryptoAnima + RemoteAnima for
  browser, M9-blocking) — depends on D-Sub-B's `RemoteAnima`
  pattern. Could parallelize with D-Sub-D (TpmAnima) + D-Sub-E
  (SomaCustody + rotation/revocation flow).

PR #1072 + #1073 merged 2026-05-01.

### 2026-05-02 — Wave 2B: D-Sub-D + D-Sub-E + D-Sub-F (3 custody backends in parallel)

Three Spec D sub-phases shipped in a single day (2026-05-02), all
parallel-dispatched on the foundation laid by Wave 1 + 2A. Each was a
distinct production custody backend; together they take Spec D from
33% to 83% complete (5/6 sub-phases). Only D-Sub-C (browser path)
remains and that is M9-blocking.

**D-Sub-D — `TpmAnima` (PR #1075, commit `27dd0fad`):** PKCS#11-backed
TPM custody for desktop deployments (mission-control on Linux/macOS).
Auth half is P-256 in TPM via `cryptoki` crate + `CKM_ECDSA` over
SHA-256 prehash; the TPM never reveals the scalar. Wallet half is
delegated (TPM 2.0 secp256k1 support is OPTIONAL/rare) to an
`Option<Arc<dyn AnimaCustody>>` — canonical mission-control deployment
pairs TPM-auth with `HardwareWalletAnima` (Ledger) for the wallet
half. Rotation is operator-driven via `C_GenerateKeyPair` with new
label `{auth_label}-rot-{ulid}`. Constructor errors bubble cleanly —
no fallback to `InProcessAnima` would silently downgrade the security
guarantee. Ships under `kms-tpm` Cargo feature (default off). Tests:
2 ignored live softhsm fixtures + 12 unit tests + integration suite.
Code-quality fixes pushed in commit `3f1cacf1`: dropped dead
`auth_public_pem: OnceLock<String>` field, removed redundant
`unsafe impl Send for TpmSession` (cryptoki Session is natively Send;
only Sync needs unsafe), fixed `rotate()` to reuse parent `Pkcs11`
(was double-initializing — most modules reject
`CKR_CRYPTOKI_ALREADY_INITIALIZED`), wrapped `pin_str` in
`zeroize::Zeroizing` to prevent plaintext PIN leak on rotate failure.
Refs Spec D L4-D5..D10. Closes BRO-XXX (D-Sub-D).

**D-Sub-F — `HardwareWalletAnima` (PR #1074, commit `8a487041`):**
Ledger-via-hidapi wallet-only wrapper. Auth-half forwards to a wrapped
`Arc<dyn AnimaCustody>` delegate (Ledger doesn't expose P-256). Wallet
half goes to the hardware device — Ledger Nano X/S/S+ running the
**Ledger Ethereum app** (`app-ethereum`). APDU codes locked at
`crate::hardware_wallet::ledger::apdu` (CLA `0xE0`,
`INS_GET_PUBLIC_KEY` 0x02, `INS_SIGN_TRANSACTION` 0x04,
`INS_SIGN_EIP712` 0x0C). HID frame layout: 64-byte reports, 5-byte
header `[channel_hi channel_lo command_tag seq_hi seq_lo]` with
`channel = 0x0101`, `command_tag = 0x05`. `rotate()` returns an error
because the seed is hardware-resident and cannot be software-rotated.
Ships under `hw-wallet` Cargo feature (default off; pulls `hidapi`).
Tests: 7 integration + 6 unit (mocked `MockHidTransport`) + 1
ignored live-Ledger end-to-end. Code-quality fix in commit `f4f85d5d`
(after rebase from `f138385c`): added outer `exchange_lock: Mutex<()>`
on `RealHidTransport` held across the FULL write_apdu+read_apdu
round-trip — fixes a real concurrency bug where per-frame mutex left
a race window between write/read APDUs (concurrent `Arc<...>` callers
could interleave protocol streams). Also dropped dead `v` parameter
from `normalize_signature`, replaced 2 `assert!`s with typed errors
in `build_apdu` + `encode_derivation_path`, and rewrote
`auth_backend_kind` doc honestly. Refs Spec D L4-D5..D10. Closes
BRO-XXX (D-Sub-F).

**D-Sub-E — `SomaCustody` + rotation/revocation flow (PR #1076, commit
`944f2ba9`):** the cross-cutting sub-phase. Adds the soma admin
custody-oracle UDS (separate from kernel UDS, authn'd via SO_PEERCRED
+ `life-runtime` group membership) PLUS the journal-side helpers that
rotation + revocation depend on. New proto:
`proto/life/admin/kernel/v1/custody.proto` defining
`life.admin.kernel.v1.CustodyOracle` with 4 RPCs (`SignAuth`,
`SignWallet`, `GetAuthPubkey`, `GetWalletPubkey`). New crate paths:
`crates/life-kernel/soma/src/admin/` (peercred extractor + AdminPolicy
+ AdminAcceptor + InProcessCustodyKeys store);
`crates/anima/anima-identity/src/soma.rs` (SomaCustody full impl —
bootstrap fetches both pubkeys, sign_* methods route through tonic
UDS, `rotate()` returns helpful error pointing at
`anima-lago::write_rotation_event`);
`crates/anima/anima-identity/src/rotation.rs` (`JournalResolver` async
trait + `walk_rotation_chain` with 256-hop cycle protection);
`crates/anima/anima-identity/src/revocation.rs` (`RevocationCache`
with TTL'd negatives + permanent positives);
`crates/anima/anima-lago/src/rotation_events.rs` (event journal
helpers); `crates/lago/lago-auth/src/agent_jwt.rs::verify_jwt` (full
verifier path: alg detect → kid extract → walk chain → check
revocation → resolve DID → ES256/EdDSA verify). Ships under
`kms-soma` Cargo feature (default off; pulls tonic + hyper-util +
life-kernel-proto). Code-quality fixes in commit `5a6010e8` (after
rebase chain): IMP-1 `peercred::peer_cred(&stream)` now fail-closes
(was silently admitting root creds to a custody oracle on syscall
failure); IMP-2 `chown_socket_to_group` via `nix::unistd::chown`
(replaces silent `tracing::warn!`); IMP-11 prominent
runtime-flavor warning on `block_on` rustdoc (`block_in_place` panics
on `current_thread` runtime). Required `nix` workspace dep +
`"fs"` feature in soma's Cargo.toml. Tests: 5 new soma integration +
8 new rotation_chain integration + 11 new unit + 5 new lago-auth
verifier integration + 7 new soma admin module tests. Refs Spec D
L4-D5..D10. Closes BRO-XXX (D-Sub-E).

**Cross-cutting integration (Wave 2B):**

- All 3 custody backends now exist as feature-gated optional
  dependencies of `anima-identity` (`kms-tpm`, `hw-wallet`,
  `kms-soma`). Default builds remain slim — only opt-in deployments
  pull cryptoki/hidapi/tonic.
- BackendKind enum is `#[non_exhaustive]` and now has all 6 variants
  filled (only WebCrypto remains).
- The shared `crate::rlp` module (D-Sub-B legacy) is now consumed by
  D-Sub-F's hardware-wallet `sign_evm_tx` round-trip + D-Sub-E's
  SomaCustody fallback path. EIP-1559 + EIP-155 RLP encoding is
  finally factored properly.
- The lago-auth `verify_jwt` multi-alg dispatcher (D-Sub-E) is the
  canonical reference for downstream verifiers — broomva.tech's
  external AAP verifier should call into lago-auth via a thin HTTP
  wrapper (or adopt the same shape) so rotation-chain semantics stay
  uniform.

**Wave 2B summary:**
- Spec D = 83% complete (5/6 sub-phases shipped; D-Sub-A through F
  except D-Sub-C). All 3 backends merged in single day via parallel
  worktree-driven dispatch.
- D-Sub-C (`WebCryptoAnima` + `RemoteAnima` for browser) remains —
  M9-blocking (~7 days). Now fully unblocked since D-Sub-E ships the
  RemoteAnima reference shape via SomaCustody.
- Tests delta: ~3860 + ~12 (D-Sub-D) + ~14 (D-Sub-F) + ~36 (D-Sub-E) =
  ~3922 Rust workspace + 50 TypeScript SDK (M8 unchanged).
- Code-quality discipline: all 3 PRs surfaced real bugs in review
  (D-Sub-D's double-init Pkcs11 in rotate; D-Sub-F's per-frame mutex
  race; D-Sub-E's fail-open peercred). All fixed in-PR via
  parallel-dispatched code-quality reviews. Two-stage review pattern
  (spec compliance + code quality) is paying off.
- Critical-path next: D-Sub-C (browser path) closes Spec D and
  unblocks M9 (apps/chat migration to AnimaCustody).

PR #1075 + #1074 + #1076 merged 2026-05-02.

### 2026-05-02 — Wave 3: D-Sub-C (browser path + Rust bridge + lifegw routes) — Spec D 100% complete

Three PRs merged the same day (2026-05-02) closing the final Spec D
sub-phase. **Spec D is now 100% complete (6/6 sub-phases shipped).**
M9 (apps/chat migration to AnimaCustody) is unblocked. The wave
shipped via 3 parallel streams (R-1 Rust + T TypeScript + R-2 lifegw)
using the worktree-driven dispatch pattern proven in Wave 2B.

**Stream R-1 — `RemoteAnima` Rust backend (PR #1082, commit
`4f4394c8`):** HTTP/JSON bridge to lifegw `/anima/custody/*` for
non-browser callers (CLIs, agents-as-services, native apps). Mirrors
SomaCustody (D-Sub-E) but with reqwest-over-HTTPS instead of tonic
UDS. Caches auth + wallet pubkeys at construction; sign_* methods
route through HTTP; `rotate()` returns the journal-flow error.
`TierUserCap { token, expires_at_unix }` with `is_expired(now_unix)`
helper. `kms-remote` Cargo feature default off (pulls reqwest +
tokio). Code-quality fixes in commit `76e16411`: dual-base64 error
clarity (decoder confusion), expired-cap fail-fast (cap_token now
returns `AnimaResult<String>` with cheap-path expiry check before
issuing the request). 107 lib tests + 5 wiremock integration tests.

**Stream T — `WebCryptoAnima` TypeScript browser backend (PR #1083,
commit `93bf7687`):** Composes `PasskeyOracle` (WebAuthn-backed P-256
auth via `navigator.credentials`) with `RemoteAnimaClient` (wallet
half delegated to lifegw via fetch). DID multicodec `0x1200` →
`did:key:zDn…` byte-identical to Rust; cross-language fixtures pin
this. `SessionCap` lifecycle (15-min Tier-User cap, refresh-on-expiry,
concurrent-mint coalescing). Spec compliance fix `be55b1f5`: route
paths use `get_auth_pubkey` / `get_wallet_pubkey` (matches Stream R-1
wiremock fixtures). Code-quality fixes in same commit: response body
sanitization (`sanitizeErrorBody` truncates upstream errors to ≤200
chars + ellipsis; structured `{ code, message }` JSON shapes
preferred), `requestTimeoutMs` config (DEFAULT 30s, AbortSignal.timeout
with manual fallback), `mapFetchFailure` distinguishing TimeoutError
from generic transport, `cfg.indexedDB` doc-comment honesty fix.
128 vitest tests + 5 cross-language DID vectors.

**Stream R-2 — lifegw `/anima/custody/*` routes + `TierUserMinter` +
WS bearer subprotocol (PR #1084, commit `b082ee52`):** 6 axum HTTP/JSON
routes + `TierUserMinter` (sibling of `Tier2Minter`, same `KmsSigner`
trait, `aud="anima.user-cap"`, 15-min default TTL). Each route
verifies the bearer JWT (signature + audience + sub-binding via
`JwksCache::verify_capability_token`), enforces per-route scope
intersection (Tier-User cap with `anima.user.sign_auth` cannot call
`/sign_wallet`), and binds `claims.sub == body.user_id`. Mint + enroll
require Tier-2 audience exclusively. WS bearer subprotocol parses
`Sec-WebSocket-Protocol: bearer.<jwt>` (Tier-1 only — closes M8.2 SDK
gap for browser WS clients that can't set Authorization headers).
Strict char validation. soma admin UDS connector via `service_fn`
(per-request connect; pooling deferred). All upstream errors are
sanitized (no `tonic::Status` leaked verbatim); per-RPC deadline 10s
via `tokio::time::timeout`; `validate_user_id` rejects empty / >64
chars / control chars at gateway boundary; request bodies use
`deny_unknown_fields`. Two rounds of in-PR fixes:

1. **Security review fixes (B1+B2+I1, commits `e825b541` + `bd8ec745`):**
   The initial PR mounted the anima router OUTSIDE the AuthLayer with
   `require_bearer` doing presence-only checks — auth bypass on 6
   privileged routes. Spec-compliance review caught this as
   BLOCKING. Fix: built `JwksCache::verify_capability_token` (new
   multi-audience verifier in `auth/jwks.rs`), added `verify_bearer`
   helper, added `check_scope_and_subject` enforcing per-route scope
   + subject binding. 6 new negative integration tests
   (unverified bearer rejected with 401, expired bearer rejected,
   wrong audience rejected, scope-insufficient rejected with 403,
   user_id mismatch rejected with 403, Tier-User cap rejected from
   mint_session_cap).

2. **Code-quality follow-up fixes (I-1 through I-7, commits
   `d9e6f505` + `b9ad820b` + `24f5da36` + `fbd740f0` + `16fc91ff`):**
   Sanitize upstream errors (`sanitize_upstream` maps
   `tonic::Code` → `(StatusCode, &'static str)`; soma UDS path no
   longer leaked in 502 body — log via `tracing::warn!`), per-RPC
   deadline (10s), `deny_unknown_fields` on request bodies,
   `validate_user_id` at gateway boundary, WS subprotocol bearer
   strict char validation. 4 new tests (deny_unknown_fields rejection,
   empty/oversized user_id rejection, WS bearer with whitespace
   rejection).

199 lifegw tests total (151 unit + 19 anima_custody integration + 29
others). Graceful degradation: if `cfg.anima_custody = None` the
routes return 501 with helpful message — lifegw still starts cleanly.

**Cross-cutting integration (Wave 3):**

- The 6 `/anima/custody/*` route shapes are now the canonical browser
  custody contract. Stream R-1's wiremock fixtures and Stream T's
  msw-mock-fetch tests pin both sides; Stream R-2 implements the
  server-side handlers. Wire compatibility verified at every boundary.
- Closes M8.2 SDK known-limitation (browser-auth via WS subprotocol
  unsupported by gateway). M8.1 (Connect-vs-grpc-web mismatch)
  remains open but is sidestepped for custody — the `/anima/custody/*`
  routes are HTTP/JSON, NOT gRPC-web, so they're consumed via plain
  `fetch` from chatOS without proto codegen.
- The lago-auth `verify_jwt` multi-alg dispatcher (D-Sub-E) and the
  new lifegw `JwksCache::verify_capability_token` (D-Sub-C R-2) both
  implement multi-audience verification — they're sibling
  implementations of the same canonical pattern. broomva.tech's
  external AAP verifier should adopt the same shape.

**Wave 3 summary:**
- Spec D = **100% complete (6/6 sub-phases shipped)**. All 6
  production custody backends ship: InProcessAnima, VaultTransitAnima,
  TpmAnima, SomaCustody, HardwareWalletAnima, WebCryptoAnima +
  RemoteAnima (paired).
- M9 (apps/chat migration to AnimaCustody) is now unblocked.
- M8.2 SDK known-limitation closed (subprotocol bearer auth).
- Tests delta: ~3922 Rust + 50 TS → ~3922 + ~12 (R-1) + ~199 net
  lifegw delta (R-2) + ~78 TS delta (T 50→128) = ~4133 Rust + 128 TS.
- Code-quality discipline: 2 streams surfaced BLOCKING issues in
  spec review (T's wire-shape divergence, R-2's auth bypass + scope
  bypass). Both fixed in-PR via dispatched fix-agents. The two-stage
  review pattern + parallel worktree dispatch + fix-in-PR discipline
  is now well-tested across 9 PRs across 3 waves.
- Critical-path next: M9 (apps/chat migration). chatOS settings UI
  for passkey enrollment + custody status + rotation history lives
  here; live USDC-transfer e2e on Base testnet validates the full
  D-Sub-C → soma → Vault → Base path.

PR #1082 + #1083 + #1084 merged 2026-05-02.

### 2026-05-18 — Spec J Phase 1: lifegw Anthropic Messages edge route (5/6 sub-phases on `main`; E2E smoke prep ready)

**Spec J = Phase 1 (5/6) shipped 2026-05-18; live E2E operator smoke
prep ready (BRO-1146).** Six PRs merged across J-Sub-A..F land the
full Claude Code interoperability surface at lifegw's edge. Phase 1
is now complete from a code-on-main perspective; the remaining
deliverable is the operator-driven live smoke against a real
Railway-deployed lifegw (BRO-1146 prep PR ships the in-process E2E
test scaffold + runbook + deploy script — the live deploy + Claude
Code session + Loom recording are human-only steps the operator
executes).

**Phase 1 components on `main`:**

- `crates/life-runtime/lifegw-anthropic-codec/` — edge-only,
  substrate-free Anthropic Messages SSE codec (encoder, block_policy,
  request, sid, state, thinking, tools, contracts, errors).
  Workspace-internal per L10-D4; not published.
- `crates/life-runtime/lifegw/src/services/anthropic_messages.rs`
  (~2,036 LOC) — the production router. Three routes: POST
  `/v1/messages`, GET `/v1/models`, POST `/v1/messages/count_tokens`.
  Real auth + rate-limit + haima check + vigil span + tonic-upstream
  wire-up.
- `crates/life-runtime/lifegw/tests/anthropic_messages_integration.rs`
  (~1,718 LOC) — full unit + integration coverage of every public-facing
  surface: SSE order, auth posture, rate-limit, x402 challenge,
  tool-use round-trip via codec, count_tokens estimator, vigil span
  emission. 4 `#[ignore]` placeholders flag the test-mock vs unfold
  race the live J-Sub-G smoke certifies (settle-on-Finish + drop+resume
  `from_sequence` replay).

**Sub-phase summary:**

| Sub-phase | Owner | Surface | Status |
|---|---|---|---|
| J-Sub-A | codec scaffold | encoder, sid, block_policy, errors | Shipped |
| J-Sub-B | lifegw route | POST `/v1/messages` + auth + tonic dial | Shipped |
| J-Sub-C | sid module | folded into J-Sub-A's `sid.rs` | Shipped |
| J-Sub-D | tool-use bridge | encoder tool_use blocks + HTTP semantics | Shipped |
| J-Sub-E | vigil + haima | spans + per-call billing + x402 (BRO-1144 PR #1335 commit `f4963feb`) | Shipped |
| J-Sub-F | models + count_tokens | static catalogue + edge estimator | Shipped |
| **J-Sub-G** | **E2E smoke (BRO-1146)** | **in-process test + operator runbook + deploy script** | **PREP READY (this PR); live operator smoke pending** |

**In-process E2E test (this PR's deliverable):**

`crates/life-runtime/lifegw/tests/spec_j_e2e_smoke.rs` — five
scenario tests against a mock lifed + real codec + real route + real
auth + recording haima:

1. `e2e_simple_chat` — full happy-path SSE shape + saga firing + sid
   stability + haima check (settle assertion deferred to live smoke
   per the documented test-mock vs unfold race).
2. `e2e_tool_use_round_trip` — first turn emits `tool_use` content
   block, second turn re-injects `tool_result` with deterministic sid
   reuse.
3. `e2e_models_endpoint` — `/v1/models` returns Anthropic-pinned
   static catalogue with ≥ 5 required IDs.
4. `e2e_count_tokens` — `/v1/messages/count_tokens` returns plausible
   count + `X-Life-Cost-Estimate-Usd-Micros` header.
5. `e2e_drop_sid_stability` — drop response body mid-stream,
   re-request, sid is deterministic via codec synthesis
   (`from_sequence` replay is the live smoke's lago-side
   responsibility; the rename from `e2e_drop_resume` was an honesty
   fix in the P20 fix-round 1 — the test does not exercise replay).

All 5 pass green. Joined with the existing
`anthropic_messages_integration.rs` tests, the lifegw test surface
now covers Phase 1 end-to-end at the in-process level.

**Three test paths for Phase 1:**

- **Path 1 — in-process E2E** (`cargo test -p lifegw --test
  spec_j_e2e_smoke`): the 5 scenarios above. Automated regression gate.
- **Path 2 — Railway staging deploy**
  (`docs/conformance/2026-05-18-claude-code-smoke-runbook.md`): live
  Claude Code session against a Railway-hosted lifegw+lifed stack.
  Produces the Phase 1 conformance evidence (Loom + Vigil traces + lago
  replay + haima ledger).
- **Path 3 — local interactive smoke** (`cargo run -p lifegw --example
  local_smoke`, BRO-1165): boots the real lifegw router against an
  in-process mock lifed UDS on a kernel-picked `127.0.0.1` port. Prints
  the URL + dev bearer and serves until SIGINT. Same code paths as
  production (codec, auth, rate-limit, Vigil) — only the substrate is
  mocked. Used for iterating on the Anthropic Messages route, validating
  Vigil span emission, and verifying the wire over a real TCP socket
  without burning a Railway slot. See
  `crates/life-runtime/lifegw/examples/README.md` for the full curl
  recipes.

A fourth iteration-only surface (`cargo run -p lifegw --example
local_smoke_anthropic`, [BRO-1185]) lands as a follow-up: same shape
as Path 3, but the in-process Agent service forwards to
`arcan_proxy::AnthropicArcan` so every `/v1/messages` call hits
`api.anthropic.com` for real (requires `ANTHROPIC_API_KEY`; defaults
to Sonnet 4.5 — switch to `claude-haiku-4-5-20251001` for cheap
iteration). Daily dogfooding loop; does NOT validate the lifed saga
(use Path 2 for full conformance evidence). See the runbook §"Path 4"
callout and `crates/life-runtime/lifegw/examples/README.md` for full
recipes and the honest divergence notes.

**Operator runbook + deploy script (this PR's other deliverables):**

- `docs/conformance/2026-05-18-claude-code-smoke-runbook.md` — seven
  sections covering prereqs, Railway staging deploy, Claude Code
  config, smoke session checklist, evidence capture, acceptance
  criteria (with the BRO-1144 carry-over known-limitations log), and
  filing the results. Uses the `lifegw-stack` multi-process
  Dockerfile (Option A, single-container Topology A with lifed running
  alongside lifegw under tini); §6.1 records mock-substrate gaps as
  expected limitations.
- `scripts/deploy_lifegw_staging.sh` — idempotent Railway deploy
  wrapper. Pre-flight (CLI installed, logged in, project linked,
  service exists via `railway service link`, branch sanity), then
  edits the baked `deploy/railway/lifegw-stack/lifegw.toml` in place
  to set `dev_signer_enabled = true` (lifegw reads TOML only, not
  env vars — see runbook §2.2a). Sets a small set of Railway-level
  env vars Railway *does* consume (`RAILWAY_DOCKERFILE_PATH`,
  `LIFEGW_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`,
  `LIFED_ALLOW_MOCK_FALLBACK`). Triggers `railway up --detach`, polls
  `/healthz` for 5 minutes, and **bails on health-probe failure**
  (no warn-and-exit-0 silent successes). Prints next-step exports
  and reminds the operator to revert the TOML edit before
  committing.

**Known limitations carried from BRO-1144 PR #1335 (documented in
the runbook §6.1 known-limitations log):**

- `traceparent` not yet propagated to lifed (W3C tracecontext is
  emitted in lifegw's spans; lifed-side spans appear as a separate
  trace until the Phase 2 follow-up wires propagation).
- `StubHaimaClient` is wired — billing is unwired at the code level
  (no `cfg.billing.enforce` field exists in the baked TOML). Ledger
  is empty across the smoke. Live haima client lands post-Phase-1
  under BRO-1147+.
- `lago replay --tree` evidence is mock-substrate-empty under
  Option A's `LIFED_ALLOW_MOCK_FALLBACK=true` posture. Real lago
  replay rides Option B (Topology B production-faithful deploy) and
  is exercised by post-Phase-1 work.

**Critical path next:** operator executes BRO-1146 live smoke per
the runbook → Phase 1 SHIPPED; then queue Phase 2 (J-Sub-H/I/J)
gated on Spec E E-Sub-F conformance completion.

**2026-06-11 — lifegw-stack Stage 6: real lago substrate (BRO-1464).**
lagod gains a substrate-plane UDS server (`--uds-socket` / env
`LAGO_UDS_SOCKET`, mirroring arcand's serve shape) — BRO-1017 had left
`lago.v1.LagoSubstrate` TCP-only while lifed's lago-proxy dials a Unix
socket, so real lago was unselectable. The lifegw-stack container now
builds + ships `lagod`, starts it before lifed (socket presence is
sampled once at lifed boot), persists its journal/blobs under the
volume-backed `${LIFE_STATE_DIR}/lago`, and pins lagod's TCP planes
clear of Caddy's `$PORT` (HTTP default was 8080 — a boot-order
collision hazard). Expected prod boot line:
`substrates: arcan=real lago=real haima=mock anima=mock`. haima/anima
stay mocked pending their daemons (incremental rollout per #1695).

**2026-06-11 — kernel dispatch path surfaces client tool definitions
(BRO-1463, completes the #1697 arc).** Stage-5's real-arcand activation
had regressed chat client tools: the gateway (mock) path attached
`tool_definitions` since #1697, but arcand's kernel/UDS dispatch
received and dropped them (the documented substrate.rs follow-up).
Now: `ClientToolDefinition` (aios-protocol, strict wire parse —
malformed entries warn+skip) rides `TickInput.client_tools` into the
provider request on every tick of a dispatch; registry tools win name
collisions; a model proposal of a client tool emits
`ToolCallRequested(category="client")` → wire `TOOL_CALL_PENDING` and
ends the turn cleanly (no kernel policy/harness involvement — the chat
surface executes the tool and continues via replayed history).
Covered by aios-protocol parse tests (7, incl. strict field-type
rejection), aios-runtime tick tests (4, incl. mixed-completion
AskHuman preservation), and a topology-B e2e (defs → provider request
→ TOOL_CALL_PENDING(category=client) → FINISH, no TOOL_RESULT, exactly
one provider call) — net +12 tests on the arc. Gap closed:
"kernel dispatch drops client tool_definitions" (BRO-1463).

## Health Summary
## Health Summary

| Area | aiOS | Arcan | Lago | Autonomic | Praxis | Vigil | Spaces |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Build | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| Tests | PASS (96) | PASS (492+16 w/ spacetimedb) | PASS (349) | PASS (219 targeted) | PASS (90) | PASS (26+2 ignored) | N/A (0 tests) |
| Clippy (-D warnings) | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| Canonical Port Usage | ACTIVE | CONSUMED | CONSUMED | CONSUMED | CONSUMED | CROSS-CUTTING | BRIDGED (arcan-spaces) |
| Production Runtime Path | CANONICAL | CANONICAL HOST | CANONICAL STORE | ADVISORY | TOOL ENGINE | OBSERVABILITY | NETWORKING |

Validation gates currently pass:

- `/Users/broomva/broomva.tech/life/aiOS`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/arcan`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/lago`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/autonomic`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/praxis`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`
- `/Users/broomva/broomva.tech/life/vigil`: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`
- `/Users/broomva/broomva.tech/life/spaces`: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo check` (WASM module: `cargo check --target wasm32-unknown-unknown --manifest-path spacetimedb/Cargo.toml`)
- `/Users/broomva/broomva.tech/life`: `make audit`, `./scripts/architecture/verify_dependencies.sh`, `./conformance/run.sh`

## Canonical Architecture

### Hard Invariants

1. `aiOS` core crates do not depend on Arcan or Lago implementation crates.
2. Lago core crates do not depend on Arcan crates.
3. Runtime boundary data uses canonical protocol types (`EventRecord`, `EventKind`, protocol IDs, canonical state).
4. Persistence writes go through canonical event-store port implementations.
5. Canonical session API is the public runtime API family.

### Canonical Session API

- `POST /sessions`
- `POST /sessions/{session_id}/runs`
- `GET /sessions/{session_id}/state`
- `GET /sessions/{session_id}/events`
- `GET /sessions/{session_id}/events/stream`
- `POST /sessions/{session_id}/branches`
- `GET /sessions/{session_id}/branches`
- `POST /sessions/{session_id}/branches/{branch_id}/merge`
- `POST /sessions/{session_id}/approvals/{approval_id}`

## Project Status

## aiOS

### Canonical Contract

- `aios-protocol` exports canonical runtime ports:
  - `EventStorePort`
  - `ModelProviderPort`
  - `ToolHarnessPort`
  - `PolicyGatePort`
  - `ApprovalPort`

### Runtime

- `aios-runtime` is port-driven and decoupled from concrete Arcan/Lago internals.
- Supports branch-aware event sequences, run lifecycle events, policy/approval flow, and state/homeostasis update emission.
- Supports explicit session creation and named-session bootstrapping used by canonical hosts.

### Composition

- `aios-kernel` composes runtime + ports.
- `aios-events`, `aios-policy`, `aios-memory`, `aios-tools` align to canonical port interfaces.

## Arcan

### Host + Adapters

- `arcan` binary hosts `aios-runtime` as production runtime path.
- `arcan-aios-adapters` implements canonical provider/tool/policy/approval/memory ports.
- `arcand` serves the canonical session API router.
- Reasoning observability is active on the canonical host path:
  knowledge bootstrap emits typed retrieval events at session spawn, and
  a kernel turn middleware derives typed knowledge observability events from
  `wiki_search` / `wiki_lint` tool completions without coupling persistence
  into the tool trait itself. Run completion now also moves through a typed
  observer payload so post-run evaluators consume canonical assistant/tool/
  knowledge context instead of re-deriving ad hoc metadata.

### Runtime Surface

- Active `arcand` module surface is canonical-only.
- Canonical API integration tests cover:
  - session lifecycle
  - named-session run auto-create behavior
  - streaming replay framing (including Vercel AI SDK v6 data envelope/header path)

### Spaces Bridge

- `arcan-spaces` provides port-based abstraction (`SpacesPort` trait) for Spaces networking.
- 6 tool definitions: list_channels, send_message, read_messages, send_dm, create_channel, list_members.
- Middleware for agent event logging to Spaces channels.
- Mock hub for testing (18 tests).
- **SpacetimeDB HTTP adapter** (`spacetimedb` feature): Concrete `SpacetimeDbClient` implementing `SpacesPort` via SpacetimeDB REST API (SQL reads + reducer calls). 16 new tests (+2 ignored live integration). Backend selection via `--spaces-backend spacetimedb` or `ARCAN_SPACES_BACKEND` env var.

### Client Alignment

- `arcan-tui` uses canonical session + approval endpoints.
- Stream parsing supports canonical event records and canonical Vercel AI SDK v6 wrapper payloads.

## Lago

### Canonical Persistence

- `lago-aios-eventstore-adapter` implements canonical `EventStorePort` over `lago_core::Journal`.
- Canonical conversion path uses `lago_core::protocol_bridge`.
- Branch-local monotonic sequencing remains enforced by journal semantics.

### Substrate

- Journal, blob store, policy engine, API, and file/manifest subsystems are operational and tested.

### Context Engine (2026-03-19)

- 12 crates total (was 10): added `lago-knowledge` (143 tests) and `lago-auth` (5 tests).
- `lago-knowledge`: YAML frontmatter parsing, `[[wikilink]]` extraction, in-memory knowledge index, scored search (+2 name, +1 body, +1 tag), BFS graph traversal.
- `lago-knowledge`: also now includes EGRI calibration substrate —
  typed benchmark schema/runner, a seed benchmark corpus, parameterized BM25
  tuning surface, `KnowledgeThresholdArtifact` bounds/validation, and a
  deterministic `KnowledgeThresholdProposer`, `KnowledgeQualityEvaluator`,
  `KnowledgeTrialExecutor`, and `KnowledgeCalibrationCampaign` for bounded
  calibration candidates, immutable composite scoring, evaluator-ready trial
  execution, full campaign orchestration, and governed promotion to the
  `lago.toml` `[knowledge]` section with versioned rollback metadata plus
  `egri.knowledge.promoted` audit events.
- `lago-knowledge` traversal resolution now accepts exact paths, relative
  paths, path stems, and wikilink syntax for graph starts. `arcan-lago` shapes
  those traversal primitives into `MemoryGraphResponse`, and Arcan shell
  exposes the read-only `memory_graph` tool beside the existing agent-driven
  memory retrieval tools.
- Autonomic: EGRI rollback monitoring folds promoted knowledge threshold
  versions and regression counters, emits durable `autonomic.RollbackRequested`
  advisories after sustained post-promotion health regression, and marks the
  active promotion as handled once the rollback request is folded.
- `lago-auth`: JWT validation (HS256 shared secret), axum auth middleware, user→session mapping (`vault:{user_id}`).
- `lago-api`: Auth-protected `/v1/memory/*` routes (manifest, file CRUD, search, traverse, note resolution).
- `lagod`: `LAGO_JWT_SECRET` env var or `[auth]` TOML section. Session map rebuilt on startup. Backward-compatible when no secret set.
- `lago-cli`: 7 `lago memory` subcommands (status, ls, search, read, store, ingest, delete). Token from `BROOMVA_API_TOKEN` env or `~/.broomva/config.json`.
- Full workspace: 371+ tests passing, 0 clippy warnings.

## Governance and Dependency Control

Architecture dependency gate is active:

- Script: `/Users/broomva/broomva.tech/life/scripts/architecture/verify_dependencies.sh`
- Integrated in: `make audit`
- Audit enforcement path:
  - `/Users/broomva/broomva.tech/life/Makefile.control`
  - `/Users/broomva/broomva.tech/life/scripts/audit_control.sh`

## Conformance Coverage

Conformance harness entrypoint:

- `/Users/broomva/broomva.tech/life/conformance/run.sh`

Current suite validates:

1. Protocol contract checks (35 tests).
2. Arcand canonical session API behavior (9 tests: lifecycle, auto-create, streaming, cursor invariants, branch isolation, merge round-trip).
3. Arcan-Lago replay/bridge behavior (3 tests).
4. Lago journal sequence assignment semantics (1 test).
5. Lago API session/SSE behavior (8 tests).
6. Lago-aiOS eventstore adapter bridge checks (11 tests).
7. Lago journal golden replay tests (14 tests: simple-chat, tool-round-trip, branch-fork, branch-merge, forward-compat, forward-compat-evolution).

## Autonomic

### Homeostasis Controller

- Three-pillar regulation: operational, cognitive, economic homeostasis.
- 5 crates: `autonomic-core` (51 tests), `autonomic-controller` (140 tests), `autonomic-lago` (8 tests), `autonomic-api` (18 tests), `autonomicd` (2 tests).
- Pure rule engine with deterministic projection fold over events.
- Economic modes: Sovereign, Conserving, Hustle, Hibernate — with hysteresis-gated transitions.
- Dual-mode advisory architecture:
  - **Embedded** (default): In-process `autonomic-controller` fold+rules with microsecond-latency gating; no network required.
  - **Remote** (opt-in via `--autonomic-url`): Consults standalone daemon via HTTP GET `/gating/{session_id}`; failures are non-fatal.
- Economic gate handle wired to provider layer: Hibernate blocks model calls, Hustle caps tokens.
- Token usage flows through RunFinished events → event mapping → Autonomic fold.
- Typed knowledge observability now flows through the same fold:
  `KnowledgeSearched` increments search volume,
  `KnowledgeRetrieved` accounts for injected context-token cost, and
  `KnowledgeEvaluated` updates knowledge health and indexed-note count.
- Lago journal integration via `--lago-data-dir` flag; on-demand session bootstrapping.

### Integration Points

- Depends on `aios-protocol` (canonical contract) and `lago-core`/`lago-journal` (persistence).
- Events use `EventKind::Custom` with `"autonomic."` prefix for forward-compatible Lago persistence.
- `arcan-aios-adapters` depends on `autonomic-core` and `autonomic-controller` for embedded mode.
- Does not depend on Arcan crates — standalone advisory service.

### Known Gaps

- Not yet consulted by Arcan agent loop (R5 Phase 1 COMPLETE — `AutonomicPolicyAdapter` decorator wired in Arcan).
- Feedback loop open (Autonomic projection always at default) (R5 Phase 2 COMPLETE — embedded controller, economic gating, token usage flow).
- No observability (metrics/traces) yet.
- Identity system is placeholder.

## Praxis

### Canonical Tool Execution Engine

- Standalone tool execution and sandbox engine extracted from `arcan-harness`.
- 4 crates: `praxis-core` (21 tests), `praxis-tools` (24 tests), `praxis-skills` (11 tests), `praxis-mcp` (34 tests).
- **90 tests total** across all crates.
- Depends only on `aios-protocol` — no dependency on Arcan, Lago, or Autonomic.
- Implements canonical `Tool` trait from `aios-protocol::tool`.

### Components

- **praxis-core**: Sandbox policy enforcement, workspace boundary checks (FsPolicy), FsPort abstraction (pluggable filesystem), command runner abstraction.
- **praxis-tools**: ReadFile, WriteFile, ListDir, Glob, Grep, EditFile (hashline/Blake3), Bash, ReadMemory, WriteMemory.
- **praxis-skills**: SKILL.md frontmatter parser, skill registry with discovery and activation.
- **praxis-mcp**: Full MCP server + client bridge via rmcp 0.15.
  - **Server**: `PraxisMcpServer` (`ServerHandler`) exposes any `ToolRegistry` as an MCP server.
  - **Transports**: stdio (Claude Desktop/CLI) and Streamable HTTP (axum) with session management.
  - **Client**: `connect_mcp_stdio()` connects to external MCP servers via subprocess.
  - **Bridge**: `McpTool` wraps external MCP tools as canonical `Tool` trait implementations.
  - **Conversions**: Bidirectional canonical ↔ MCP type mapping (definitions, results, annotations, content).
  - **Tests**: 24 unit + 9 integration + 1 doctest, including full MCP protocol roundtrip via duplex transport.

### Integration Points

- Depends on `aios-protocol` (canonical tool contract).
- Consumed by Arcan via `arcan-harness` bridge (Praxis is the canonical tool backend).
- Architecture dependency audit enforces isolation from Arcan/Lago/Autonomic.

### Known Gaps

- Not yet wired into Arcan (arcan-harness now bridges to Praxis tools).
- No integration tests with live external MCP servers (roundtrip tests use in-process duplex transport).

## Vigil

### Observability Foundation

- OpenTelemetry-native tracing and GenAI metrics for the Agent OS.
- 1 crate: `vigil` (56 tests + 2 ignored).
- Depends only on `aios-protocol` — no dependency on Arcan, Lago, Autonomic, or Praxis.
- Implements contract-derived spans (EventKind → OTel spans), GenAI semantic conventions (`gen_ai.*` attributes), and dual-write architecture (OTel spans + EventEnvelope trace context).

### Components

- **config**: `VigConfig` with env var overrides (`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`, `VIGIL_LOG_FORMAT`, `VIGIL_CAPTURE_CONTENT`, `VIGIL_SAMPLING_RATIO`).
- **semconv**: GenAI semantic conventions (`gen_ai.*`), Life attributes (`life.*`), Autonomic attributes (`autonomic.*`), Lago attributes (`lago.*`).
- **envelope/jsonl/pricing**: typed LLM call envelope, best-effort JSONL dual-write, and local model-pricing snapshot for response-side cost estimation.
- **spans**: Contract-derived span builders (`agent_span`, `phase_span`, `chat_span`, `tool_span`), knowledge-operation spans (`knowledge.context_build`, `knowledge.search`, `knowledge.lint`), typed LLM envelope attributes, and trace context helpers (`current_trace_context`, `write_trace_context`, `extract_trace_context`).
- **metrics**: `GenAiMetrics` — OTel instruments for token usage, operation duration, LLM request outcomes, estimated cost, tool executions, budget gauges, and mode transitions.

### Integration Points

- Depends on `aios-protocol` (canonical contract — `EventEnvelope`, `LoopPhase`, `TokenUsage`).
- Consumed by Arcan provider adapters for chat-span enrichment, aggregate GenAI/Vigil metrics, local JSONL LLM call artifacts, and serialized cost-envelope persistence through `ModelCompletion`.
- Graceful degradation: structured logging via `tracing-subscriber` when no OTLP endpoint is configured.

### Known Gaps

- No OTLP smoke test in CI.
- OpenAI-compatible and Anthropic provider paths populate finish reason, and populate time-to-first-token where streaming exposes a first content/tool delta. OpenAI-compatible non-streaming calls also populate retry counts from the provider retry loop. Fallback and circuit-breaker fields remain present but defaulted until a routing/circuit subsystem owns those decisions. PII and redaction fields remain schema-ready but are not yet populated by a sanitizer.

## Spaces

### Distributed Agent Networking

- SpacetimeDB 2.0 WASM module providing real-time distributed communication for agents.
- 11 tables, 20+ reducers, 5-tier RBAC (Owner/Admin/Moderator/Member/Agent).
- Channel types: Text, Voice, Announcement, AgentLog.
- Message types: Text, System, Join, Leave, AgentEvent.
- Rust CLI client with 26 commands using `spacetimedb-sdk`.
- Auto-generated client bindings (44 files) via `spacetime generate`.

### Integration Points

- Standalone project — does not depend on aiOS/Arcan/Lago crates.
- Arcan agents will connect as SDK clients for distributed coordination.
- AgentLog channels and AgentEvent messages provide agent-specific communication primitives.

### Known Gaps

- No unit tests (reducer tests, integration tests planned).
- No DM/private messaging.
- arcan-spaces bridge uses mock hub only — concrete SpacetimeDB SDK adapter not yet implemented. (SpacetimeDB HTTP adapter COMPLETE — `SpacetimeDbClient` via REST API with backend selection).

## Architecture Scorecard

- Agent loop: 9/10 | Persistence: 10/10 | Tool harness: 9/10
- Memory: 8/10 | Context quality: 9/10 | Self-learning: 2/10 — EGRI substrate wired (autoany-aios + autoany-lago adapters), cross-run inheritance available. No live self-improvement loop yet.
- Observability: 2/10 | Security: 4/10 | Operational tooling: 8/10

---

## Remaining Work (Post-Baseline)

The baseline runtime architecture is in place and validated. Remaining work is additive:

1. Cross-project golden fixture expansion for replay determinism breadth (R1, COMPLETE — branch-merge, forward-compat-evolution, stream cursor/replay invariants).
2. Observability depth expansion (metrics/traces across runtime and adapters) (R2, FOUNDATION COMPLETE — Vigil crate with OTel tracing, GenAI metrics, contract-derived spans; integration into runtime projects pending).
3. Security hardening beyond current software-level sandbox controls (R3, PLANNED).
4. Memory and learning depth (R4, PLANNED).
5. Controller plane / Autonomic integration — Phase 0 COMPLETE (5 crates, 69 tests, Lago wired, hysteresis active); Phase 1 COMPLETE: Arcan advisory client wired (`AutonomicPolicyAdapter` decorator, 6 tests); **Phase 2 COMPLETE**: Embedded controller (dual-mode adapter, economic gate handle wired to provider, token usage flow, 24 new tests — R5 DONE).

### Infrastructure (2026-03-01)

- [x] Root PLANS.md created for execution tracking.
- [x] docs/control/ARCHITECTURE.md expanded (was stub).
- [x] docs/control/OBSERVABILITY.md expanded (was stub).
- [x] Recovery script (scripts/control/recover.sh) upgraded with diagnostics.
- [x] CLI E2E tests wired (scripts/control/cli_e2e.sh exercises lago-cli, lagod, arcan).
- [x] Web E2E tests wired (scripts/control/web_e2e.sh exercises arcand HTTP API).
- [x] CI workflows updated for CLI and Web E2E pipelines.
- [x] MemoryPort removed from canonical port list (was removed from aios-protocol 2026-02-28).

## Baseline Completion Checklist

- [x] Single canonical contract (`aios-protocol`) across projects.
- [x] Single canonical runtime engine (`aios-runtime`) in production host path.
- [x] Lago-backed canonical persistence adapter in active runtime path.
- [x] Canonical session API routed by `arcand` and hosted by `arcan`.
- [x] Architecture dependency gate integrated in audit flow.
- [x] Workspace build/lint/test gates green.
- [x] Conformance harness green.
