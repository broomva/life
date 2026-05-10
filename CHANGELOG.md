# Changelog

## Unreleased

### Added
- **Bookkeeping authored agents** — four new blessed-tier agents at
  `agents/bookkeeping-{novelty,specificity,relevance,synthesizer}.md`
  validating the use case for the Nous gate (P8 scoring pipeline) as
  authored agents rather than embedded prompts. (BRO-1012)
  * `bookkeeping-novelty.md` — single-shot Novelty scorer (0–3).
    Inputs: `{item_text, source_type, existing_entity_slugs?,
    project_modules?}`. Output: `{score, closest_existing_slug,
    reasoning, anti_pattern_warnings[]}`. Implements the rubric in
    `skills/bookkeeping/references/scoring-rubric.md` §1 verbatim.
  * `bookkeeping-specificity.md` — single-shot Specificity scorer
    (0–3). Output adds a `concrete_evidence[]` array of verbatim
    extracts from the input that justify the score, capped at 0
    iff none.
  * `bookkeeping-relevance.md` — single-shot Relevance scorer (0–3).
    Inputs include `active_projects[]`, `open_questions[]`,
    `archived_or_paused_projects[]`. Output captures
    `connected_projects[]` + (iff score=3) the verbatim
    `addresses_open_question` text.
  * `bookkeeping-synthesizer.md` — multi-turn Layer-4 synthesizer
    (max 8 turns). Inputs: `{topic, entities[≥3], audience}`.
    Outputs structured markdown sections with `[[type/slug]]`
    wikilink citations. Self-flags `blog_post_candidate` against
    a high bar.
  * **Cross-agent invariant**: the three scorers share output shape
    (`{score, reasoning, anti_pattern_warnings, …}`) so P8 can fan
    out the same item to all three in parallel and aggregate the
    raw score 0..=9. The synthesizer intentionally cannot cite
    entities it wasn't given (`cited_entities ⊆ input.entities`).
  * **Fixture coverage**: `crates/arcan/arcan-ergon/tests/agents_fixtures.rs`
    grew its `REQUIRED_BLESSED_AGENTS` constant from 3 to 7. The
    existing 6 fixture tests now validate all 7 blessed agents
    (FsAgentRegistry load, well-formed spec, schema compiles under
    production validator). No new tests; existing assertions cover
    the new agents uniformly.
  * **`agents/README.md`** gained a "Currently shipped (BRO-1012 —
    bookkeeping)" section documenting purpose, shape, and the
    aggregator design. Naming convention `bookkeeping-<dimension>`
    keeps related agents adjacent in directory listings.

- **`nous-tools` crate** — lineage primitives + the three praxis tools
  authored agents (BRO-1011 nous-promoter, BRO-1012 bookkeeping
  scorers) need to consume nous output. (BRO-1009)
  * **`NousLineage` trait + `LineageEvent` / `LineageFilter`** — the
    provenance contract for nous decisions: which scorer ran, with what
    input, what output, against which target, when, and (optionally)
    against which lago `EventId`. `LineageFilter` matches by session,
    agent, target, time window, and limit.
  * **`InMemoryNousLineage`** — `Arc<RwLock<Vec<_>>>`-backed reference
    implementation. Sequential `lineage-N` ids; preserves insertion
    order on `query`. The lago-backed implementation is intentionally
    deferred to a follow-up so the trait + in-memory ref ship as a
    clean unit.
  * **`nous_aggregate` praxis tool** — descriptive statistics
    (`count`, `mean`, `median`, `min`, `max`, sample-stddev) over a
    `Vec<f64>` of scores. Hand-rolled (no `statrs` / `medians`
    workspace dep for ~20 LOC of arithmetic). Empty input returns
    zeroes; `count == 1` returns `stddev == 0.0` (Bessel correction
    undefined). Rejects NaN / inf / non-numeric entries with
    `ToolError::InvalidInput`.
  * **`nous_compare` praxis tool** — typed verdict over
    `(a, b, threshold)`: `greater` / `lesser` / `equal` /
    `within_threshold`. Default threshold `0.001`. Strict equality
    returns `Equal` even inside the threshold band so the verdict
    stays informative; `threshold = 0.0` collapses to strict
    comparison.
  * **`lago_query` praxis tool** — read-only event-journal queries
    over `Arc<dyn lago_core::Journal>`. Filters by `event_kind`
    discriminant (delegated to `EventQuery::with_kind`), `after_ts_ms`
    / `before_ts_ms` window (lago stores microseconds; the tool
    exposes ms for parity with `LineageEvent`), and `limit` (default
    50, hard cap 500 to bound memory for untrusted agents). The sync
    `Tool::execute` ↔ async `Journal::read` boundary is bridged via
    a stashed `tokio::runtime::Handle` and `block_in_place` +
    `block_on` (mirrors `praxis-mcp-bridge::McpTool`); falls back to
    plain `block_on` outside an active runtime so the tool also works
    from sync callers.
  * **Tests** — 18 unit tests + 1 integration test
    (`tests/tools.rs`) that exercises all three tools end-to-end
    through the canonical `Tool::execute` path: record → query →
    aggregate → compare. The integration test pins a real `Journal`
    impl in-test (`VecJournal` over `Arc<RwLock<Vec<_>>>`) so the
    `Arc<dyn Journal>` interface is exercised.
  * **NOT in scope** — the lago-backed `NousLineage` impl and the
    BRO-1011 `nous-promoter` authored agent that consumes these tools
    land in follow-up tickets.

- **First authored agents** at `agents/<name>.md` — the Layer-3 data
  files that prove the substrate end-to-end. (BRO-1010)
  * `agents/general.md` — multi-turn generalist (max 16 turns).
    Inherits the workflow's full tool set; emits typed
    `{response, confidence, used_tools}`.
  * `agents/goal-pursuer.md` — multi-turn goal pursuer (max 32
    turns). Receives `{goal, success_criteria, constraints,
    prior_context}`; plans, executes tools, may delegate via
    `spawn_agent`; emits typed
    `{outcome, evidence, unmet_criteria, next_steps, reasoning}`
    with `outcome` ∈ {`success`, `partial`, `failure`}.
  * `agents/goal-judge.md` — single-shot judge (max_turns 1) that
    scores a `goal-pursuer` run against the original criteria.
    Outputs `{score (0..3), honest, criteria_assessment[],
    suggestions[], summary}`. The `criteria_assessment` array
    pins one verdict per success criterion (`met` / `partly_met`
    / `not_met` / `unverifiable`).
  * `agents/README.md` — the authoring contract: format, the
    filename↔name match rule, the self-validation workflow, and the
    "human-PR-authored only, no production self-modification" rule
    from architecture spec §7.3.
  * **arcan binary**: new global flag `--agents-dir <DIR>` (env
    `ARCAN_AGENTS_DIR`) defaulting to `./agents/`. On `arcan serve`
    boot the binary calls `ergon::FsAgentRegistry::load`, logs the
    loaded agent count at INFO, and falls back to an empty
    `InMemoryAgentRegistry` with a warning if the directory is
    missing — boot is never blocked by missing agents, but
    `spawn_agent` calls then fail-closed in-band with
    `unknown_agent`.
  * **`ergon::FsAgentRegistry`** now silently skips markdown files
    that lack YAML frontmatter or have an empty body — these are
    documentation files (e.g. `agents/README.md`,
    `agents/CHANGELOG.md`), not agents. Genuine YAML / schema
    errors in actual agent files still fail loud at load time.
    The previous behavior treated `README.md` as a malformed agent
    and refused to load the entire directory.
  * **Fixture tests** — `crates/arcan/arcan-ergon/tests/agents_fixtures.rs`
    runs offline (no LLM, no API key) on every `cargo test`,
    asserting per the architecture spec §M7 (prompt-fragility
    hardening):
    1. The `agents/` directory loads cleanly through
       `FsAgentRegistry`.
    2. All required blessed agents (`general`, `goal-pursuer`,
       `goal-judge`) are present.
    3. Each agent's spec is well-formed (validates,
       non-empty model/instructions, object-typed schemas).
    4. Each input/output schema compiles under the production
       `jsonschema` validator (so a malformed schema fails at test
       time, not at first inference call).
    5. `goal-pursuer.outcome` declares the concrete enum
       `{success, partial, failure}`.
    6. `goal-judge.claimed_outcome` mirrors `goal-pursuer.outcome`
       byte-for-byte (so a maintainer who edits one without the
       other fails CI rather than producing a workflow that
       silently rejects every pursuer answer).


- `ergon::StepCtx::dispatch_spawn_agent` — wires the
  `spawn_agent(name, input)` builtin tool through to actual sub-agent
  execution. Closes the substrate-to-runtime gap that BRO-1007 left as
  a fast-follow. (BRO-1007b)
  * **`StepCtx::agent_registry`** and **`StepCtx::recursion`** — two new
    optional fields (with `with_agent_registry` / `with_recursion`
    builder methods) so the host runtime can plumb a registry and
    recursion frame in per tick.
  * **`StepCtx::dispatch_tool` is now `&mut self`** — required because
    the spawn intercept calls `agent.run(&mut self, input)`. All other
    tool dispatch paths only need `&self`-equivalent access; the
    upgrade is harmless because the autonomous loop already holds
    `&mut self` and dispatches tools serially.
  * **Spawn intercept** — when the model emits a tool_use for
    `SPAWN_AGENT_TOOL`, dispatch resolves args via `SpawnArgs`,
    validates via `RecursionContext::check_can_spawn`, looks up the
    target via `AgentRegistry::get`, opens a child recursion frame via
    `mem::replace`, runs the sub-agent (`Agent::run`), restores the
    parent frame, and wraps the result as
    `ToolResult::success({ok, agent, output})` or in-band
    `ToolResult::model_error({error: <category>, detail, agent})`. All
    failure modes (unknown agent, missing registry, depth/cycle/
    invocation/budget refusal, sub-agent runtime error) surface to the
    parent agent in-band so it can adapt on the next turn rather than
    aborting the workflow.
  * **`ChainedToolRegistry::spawn_tool`** — when a registry is configured
    on `StepCtx`, `run_spec` injects a `SpawnAgentTool` into the
    chained registry so the model sees `spawn_agent` advertised in its
    tool list. Definitions de-duplicate framework tools across nested
    chain layers (the case during nested sub-agent dispatch).
  * **`arcan-ergon::WorkflowRunInputs`** gained `agent_registry`,
    `max_recursion_depth`, `max_invocations` fields (with builders
    `with_agent_registry` / `with_max_recursion_depth` /
    `with_max_invocations`); `run_workflow_as_tick` constructs a fresh
    root `RecursionContext` per tick and attaches it + the registry to
    the `StepCtx`. Recursion limits are per-tick (a tick is a bounded
    computation), never carried across ticks.
  * **arcan binary** constructs an empty `InMemoryAgentRegistry` at
    startup and attaches it via `WorkflowRunInputs::with_agent_registry`,
    establishing the wiring shape. First authored agents land in
    BRO-1010 (`agents/goal-pursuer.md` etc.) and populate this registry.
  8 new integration tests (`tests/spawn_dispatch.rs`): happy path,
  unknown agent, no registry configured, depth-limit refusal, cycle
  detection, two-level recursion, invalid arguments, sub-agent error.
  Total ergon test count: 121 passing (98 unit + 15 agent_primitive +
  8 spawn_dispatch).

- `ergon::AgentRegistry` / `ergon::SpawnAgentTool` / `ergon::RecursionContext` —
  the substrate layer that lets authored AgentSpecs be invoked by name from
  within an agent's loop. (BRO-1007)
  * **`AgentRegistry` trait** — name → `Arc<dyn Agent>` lookup. Three impls:
    `InMemoryAgentRegistry` (programmatic), `FsAgentRegistry` (loads
    `agents/<name>.md` files recursively from a root directory), and
    `ChainedAgentRegistry` (composes multiple sources in order; future
    `LagoAgentRegistry` slots in via the same trait). Filenames must match
    spec `name` field — the registry rejects mismatches at load time.
  * **`parse_agent_md`** — parses Markdown with YAML frontmatter into an
    `AgentSpec`. Frontmatter holds structured fields; body becomes
    `instructions`. Format chosen per architecture spec
    `docs/superpowers/specs/2026-05-09-bro-1006-authored-agents-architecture.md`
    §4 (Claude Code skills convention; same pattern as Anthropic / Vercel /
    every modern agent ecosystem). JSON remains the wire format for lago
    events and network transports; Markdown is the authoring surface.
  * **`SpawnAgentTool`** — builtin tool definition (synthesized into every
    workflow's `ToolRegistry`) that lets agents invoke registered sub-
    agents from within their autonomous loop. The tool definition ships
    here; the actual dispatch (with live `&mut StepCtx` access) is wired
    in a fast-follow PR. The tool emits structured
    `ToolResult::model_error` payloads on failure (unknown agent,
    recursion-context refusal) so the parent sees them in-band.
  * **`RecursionContext`** — per-tick recursion guardrail with depth
    limit (default 8), invocation count limit (default 256), token
    budget propagation, wall-clock budget propagation, and cycle
    detection (rejects `spawn_agent("foo")` if `foo` is already on the
    invocation stack). Atomic counters shared via `Arc` so siblings see
    each other's consumption. `RecursionError` variants surface as
    model-visible tool errors, never panics. Per architecture spec §6,
    this hardening is non-negotiable and ships with the spawn primitive.
  * **JSON Schema validation upgrade** — replaces the v0.1 hand-rolled
    `validate_against_schema` (structural-only) with the full
    `jsonschema` crate (draft-7 / 2019-09 / 2020-12). Now catches
    `range(min, max)`, `oneOf`/`anyOf`/`allOf`, `pattern`, `enum`,
    `format`, etc. Errors carry structured paths the model can use to
    self-correct. New test: `jsonschema_rejects_out_of_range_integer`.
  * **Workspace deps**: `jsonschema = 0.18` and `gray_matter = 0.2`
    in `[workspace.dependencies]`. ergon also gains `serde_yaml` for
    frontmatter parsing.
  31 new tests (12 recursion + 14 registry/parse + 5 builtin_tools) +
  1 new integration test for schema validation. Total ergon test count:
  113 passing (98 unit + 15 integration).

  **What this PR does NOT do** (deferred to fast-follow):
  - Wire `spawn_agent` dispatch into the autonomous loop —
    requires `&mut StepCtx` at the dispatch site, meaningful change to
    `run_inference_streaming`.
  - Plumb `AgentRegistry` through `WorkflowRunInputs` / arcan-ergon.
  - Add `arcan agent new/list/show/test` CLI tooling (BRO-1008).

- `ergon::Agent` / `ergon::AgentSpec` / `ergon::TypedAgent` — first-class
  typed-I/O agent primitive for the Life agent harness. (BRO-1005)
  * **`AgentSpec`** is data: serializable, `JsonSchema`-derivable,
    constructible at runtime, returnable as the typed `Output` of
    another agent (the "factory" pattern that enables agent-emits-agent
    composition without recursion machinery), and embeddable in
    streams or lago events.
  * **`Agent` trait** is the unifying contract: `fn spec()` + `async fn
    run(ctx, input: Value) -> Output: Value`. Implemented for
    `AgentSpec` directly (dynamic path) and auto-derived for any
    `T: TypedAgent` (static path). Both lower to the same engine —
    `ergon::run_spec`.
  * **`TypedAgent` trait** is the static-typed convenience: declare
    `Input` / `Output` as Rust types with `Serialize` + `Deserialize`
    + `JsonSchema` bounds plus a few config methods, and the framework
    auto-derives an `AgentSpec` (with sanitized JSON Schemas) and
    auto-impls `Agent` over it.
  * **`AgentStep<A>` / `TypedAgentStep<T>`** wrappers bridge agents
    into `ergon::Step` so workflow bodies compose them via the standard
    `ctx.step(&agent_step, input)` API. Explicit wrappers (rather than
    a blanket `impl<A: Agent> Step for A`) preserve trait coherence
    so future blanket Step impls remain possible.
  * **The interpreter** (`ergon::run_spec`) drives the autonomous loop
    with a synthetic `record_answer` tool injected into the workflow's
    `ToolRegistry` (via `ChainedToolRegistry`). The model's final act
    must be `record_answer({"answer": <typed value>})`; the interpreter
    captures the args via a side channel, validates against the output
    schema, deserializes, and returns. On schema violation a corrective
    user message is appended and the loop retries up to `max_retries`;
    exhausted retries surface as `AgentError::SchemaViolation`. Other
    typed failures: `AgentError::AnswerNotEmitted` (model never
    recorded), `AgentError::Refusal` (provider stop_reason=Refusal),
    `AgentError::InvalidSpec` (pre-flight validation).
  * **Sub-context isolation**: each agent invocation opens an isolated
    sub-scope via `StepCtx::swap_scope` — separate message history,
    chained tool registry, but shared provider/hooks/sink/runtime.
    This isolation is for **serial** reuse of one `StepCtx` so a
    follow-up step doesn't inherit the previous agent's conversation.
    Parallel fan-out over multiple agents (e.g. `try_join_all`)
    requires the workflow author to clone the substrate Arcs and
    construct independent `StepCtx` instances — one per branch.
  * **Forward-compat slot**: `AgentSpec.extensions: HashMap<String, Value>`
    plus `#[non_exhaustive]` lets future patterns (recursion configs,
    identity constraints, scheduling hints, remote refs, Spec E backend
    hints) land additively without breaking changes.
  * Adds `schemars = "0.8"` dependency to ergon (already in workspace).
  14 integration tests with `ScriptedProvider` covering: happy path
  single-turn, multi-turn with workflow tool dispatch, schema-violation
  retry success, retry exhaustion → typed error, refusal, answer-not-
  emitted, dynamic `AgentSpec::run` parity with TypedAgent, agent-emits-
  AgentSpec factory pattern, sub-context isolation, schema sanitization.

  **Why this shape**: an agentic OS needs primitives for identity,
  capability, state, budget, observability, trust, communication,
  discovery, and lifecycle. The Agent primitive's job is small and
  precise — only the typed-I/O contract + first-class spec value +
  agent-loop discipline. Identity/capability/state/budget/observability
  /trust are delegated to existing substrates (anima, autonomic, vigil,
  lago, nous) via the existing hook + sink + port architecture.
  Communication patterns (mailboxes, pub/sub, remote dispatch) compose
  via `AgentSpec`'s serializability without primitive changes.
  Discovery, recursion, and long-lived agents are deferred to follow-
  up primitives that compose with — rather than mutate — the Agent
  trait. The first workflow that genuinely needs in-loop spawning
  will land that as a narrow follow-up.

- `arcan-ergon` — new sibling crate at `crates/arcan/arcan-ergon/`
  delivering the kernel-side adapter that runs an `ergon::Workflow`
  as the body of a single `aios_runtime::KernelRuntime` tick. Resolves
  BRO-1001's "ergon tick-body adapter" deliverable. Modules:
  * `dispatcher` — `ErgonWorkflowDispatcher` implements the new
    `aios_runtime::WorkflowTickDispatcher` trait the kernel calls per
    `TickKind::Workflow` invocation.
  * `registry` — `WorkflowRegistry` holds typed
    `Arc<W: ergon::Workflow>` impls behind a `BoxedWorkflowExecutor`
    trait that erases the `Input`/`Output` generics so the kernel can
    address workflows by string name.
  * `provider` — `ModelProviderAdapter` wraps
    `aios_protocol::ModelProviderPort` as `ergon::Provider`,
    translating between the kernel's flat `ModelCompletionRequest`
    shape and ergon's structured `Vec<Message>` form, and synthesizing
    canonical `StreamEvent` sequences from the port's directives.
  * `tools` — `ToolHarnessAdapter` wraps `ToolHarnessPort` as
    `ergon::ToolRegistry`. Capability gating is intentionally NOT
    duplicated here — it fires on `Hook::on_pre_tool_use` via the
    dedicated capability hook so it doesn't double-trigger.
  * `runtime_handle` — `ModeRuntimeHandle` exposes a per-tick captured
    `OperatingMode` as `ergon::RuntimeHandle`.
  * `hooks` — provides `KernelCapabilityResolver` (real
    `PolicyGatePort`-backed adapter for `CapabilityResolver`, with a
    `ToolCapabilityMap` declaring per-tool capability requirements
    and fail-closed behavior on unknown tools), plus `NoopBudgetGate`
    / `NoopResponseScorer` / `NoopSoulAttester` permissive stand-ins
    for `BudgetGate` / `ResponseScorer` / `SoulAttester`. Real
    autonomic / nous / anima implementations are deliberately left
    for follow-up tickets — the BRO-1001 minimum-viable adapter ships
    the only adapter that must be functional (capability gating).
  * `runner::run_workflow_as_tick` — composes a fully-built
    `ergon::StepCtx` from a `WorkflowTickInvocation` and drives the
    workflow body, returning a typed `WorkflowTickOutcome` with the
    workflow's JSON output and the count of stream events emitted.
  16 unit tests + 4 end-to-end integration tests
  (`tests/workflow_tick_e2e.rs`) verifying the workflow tick path
  against a real `KernelRuntime` over file-backed event storage:
  workflow runs, JSON output ends up in an `ergon.workflow_output`
  `Custom` event in the journal, direct ticks still work alongside
  the dispatcher, unknown workflows surface clear errors. (BRO-1001)
- arcan binary now installs an `ErgonWorkflowDispatcher` (with an
  empty registry by default) on the kernel runtime at startup.
  Adopting daemons override this section to register their concrete
  `ergon::Workflow` impls before the runtime starts serving.
  (BRO-1001)

### Changed
- `aios_runtime::TickInput` now carries a new `kind: TickKind` field
  (default `TickKind::Direct`). `TickKind::Workflow { name, input }`
  routes the tick body through a registered
  `WorkflowTickDispatcher` (see arcan-ergon) instead of the kernel's
  built-in single-call body. Existing `TickInput` constructors must
  set `kind`; all in-tree call sites updated to `TickKind::Direct`
  for behavior parity. (BRO-1001)
- `aios_runtime::KernelRuntime` gained two builder-style methods —
  `with_workflow_dispatcher(dispatcher)` and
  `has_workflow_dispatcher()` — for hosts that want to wire a
  non-direct tick body. The dispatcher trait
  (`WorkflowTickDispatcher` / `WorkflowTickInvocation` /
  `WorkflowTickOutcome`) lives in `aios-runtime` so workflow runners
  (arcan-ergon today, future shapes later) plug in without the
  kernel taking on substrate dependencies. (BRO-1001)
- **Architectural correction (docs only, no code)**: the framing in
  `docs/superpowers/specs/2026-05-05-ergon-v0.1.md` §6 (Composition with
  arcan) and §10 (Migration plan) was structurally wrong. Both sections
  assumed ergon would replace `arcan-harness` as a parallel session-level
  runtime via an `AgentKind` trait abstraction. Reality:
  1. `arcan-harness` is a ~300 LOC tool/sandbox utility crate, not the
     agent harness. The actual agent harness is a 7-layer runtime stack
     across ~25 crates.
  2. Ergon's `Workflow::execute()` is one async fn — it cannot replace
     the long-horizon tick engine (`aios_runtime::KernelRuntime`)
     without losing per-tick journal traceability that autonomic /
     EGRI / branching depend on.
  Corrected framing: ergon is **one shape of tick body** alongside the
  existing direct-call tick body. Both compose with KernelRuntime; both
  produce per-tick events; **neither replaces anything**. Documented
  comprehensively in:
  - `docs/architecture/agent-harness.md` — new canonical 7-layer
    runtime architecture (didn't exist before)
  - `docs/superpowers/specs/2026-05-08-bro-1001-ergon-tick-body.md` —
    new corrected BRO-1001 design (supersedes §6 + §10 of the original
    ergon spec)
  - In-place SUPERSEDED markers on §6 and §10 of
    `2026-05-05-ergon-v0.1.md`
  - Updated `crates/ergon/ergon/CLAUDE.md` with the corrected
    positioning
  No ergon code changes. The trait surface, hook lifecycle, wire types,
  auto-hooks, and stream sinks are all correct as-shipped — they're
  precisely what a tick-body adapter needs. What changes is the
  positioning of BRO-1001 (the integration adapter), which becomes
  `arcan-ergon::run_workflow_as_tick(...)` rather than a session-level
  `ErgonAgentKind`. (BRO-1001)

### Added
- `ergon-life-sinks` — new sibling crate at
  `crates/ergon/ergon-life-sinks/` housing three Life-flavored
  implementations of `ergon::StreamSink`:
  * `LagoSink` — durable replay via `lago_core::Journal`. Each
    `StreamEvent` becomes an `aios_protocol::EventKind::Custom` event
    with `event_type = "ergon.stream"` and the full event JSON in
    `data`. Failures propagate as `ErgonError::Internal` (durable
    replay critical).
  * `VigilSink` — emits each `StreamEvent` as a structured
    `tracing::info!` (or `warn!` for `Error` variants) on the current
    span, target `"ergon::stream"`. Despite the name it does NOT
    depend on `life-vigil` — vigil configures the subscriber, not the
    emitter. Infallible.
  * `LifegwSink` — bounded `tokio::sync::mpsc` forwarder; consumer
    disconnect surfaces as `ErgonError::StreamClosed` and propagates
    cancellation. Default capacity 64 per spec §3.10.
  19 unit tests across the three sinks (mock `Journal` impl for Lago,
  in-memory event-flow tests for Vigil, send/receive + capacity +
  closed-receiver tests for Lifegw). Crate dependency surface: only
  `ergon`, `aios-protocol`, `lago-core`, plus standard async/serde —
  no `arcan-*`, no `praxis-*`, no `anima-*`, no `autonomic-*`, no
  `nous-*`, no `life-vigil`. Failure-semantics tiers documented in
  `crates/ergon/ergon-life-sinks/CLAUDE.md`. (BRO-999b)
- `ergon-life-hooks` — new sibling crate at `crates/ergon/ergon-life-hooks/`
  housing the four "Life-native" auto-registered hooks (spec §3.8):
  `PraxisCapabilityHook` (`on_pre_tool_use`),
  `AutonomicBudgetHook` (`on_pre_inference`),
  `NousScoreHook` (`on_post_inference`),
  `AnimaAttestHook` (`on_workflow_start` / `on_workflow_end`).
  Each hook is paired with a small **adapter trait**
  (`CapabilityResolver`, `BudgetGate`, `ResponseScorer`, `SoulAttester`)
  the hook consumes via `Arc<dyn _>` at construction. The arcan
  adapter (BRO-1001) implements those traits against `aios_protocol::PolicySet`,
  `autonomic::AutonomicGatingProfile`, `nous_core::NousEvaluator`,
  and `anima_core::AgentSoul`.
  Architectural win: the crate has **zero substrate dependencies** —
  Cargo.toml lists only `ergon` + standard async/serde. Substrate dep
  lives in BRO-1001's adapter, where it belongs. 15 unit tests across
  the four modules (mocked adapters); `ergon` itself unchanged.
  Failure semantics differ by hook: capability and budget denials are
  hard veto (`Deny` outcome); score and attestation failures are
  observe-only (`tracing::warn!` + `Continue`) — design rationale
  documented in `crates/ergon/ergon-life-hooks/CLAUDE.md`.
  Spec deviation: this crate replaces the spec's "auto-hooks live in
  ergon itself" placement (§3.8). Reason: ergon stays vendor-neutral;
  Life-specific governance hooks belong in their own crate so a future
  ergon consumer (TS port, alternate agent OS) can ship its own
  governance set without forking. (BRO-1000)
- `ergon` — Layer-2 agent-harness primitive: workflow trait + executor.
  Ships the fourth slice per spec §12: `workflow` module (`Workflow`
  trait — the user-implementation surface with typed Input/Output;
  `WorkflowExecutor` — the driver that fires `on_workflow_start` /
  `on_workflow_end` hooks around `Workflow::execute`; placeholder
  `SkillSet` trait + `EmptySkillSet` impl until praxis-skills wiring
  in BRO-1001). Ten new unit tests cover: empty-hooks execution,
  start-hook deny short-circuiting, end-hook firing on execute error,
  end-hook errors not overriding workflow result, sequential first-deny
  short-circuit, default skill-set behaviour. Three more spec deviations
  documented in `crates/ergon/ergon/CLAUDE.md`: `WorkflowExecutor`
  doesn't auto-register hooks (caller passes pre-built `StepCtx`,
  matching the BRO-1000 separation); `without_default_hooks` flag
  dropped (no longer meaningful); `SkillSet` placeholder trait until
  BRO-1001 wires real praxis-skills. Result: ergon compiles and passes
  69 unit tests with **still zero substrate dependencies**. 59 → 69
  tests. (BRO-999)
- `ergon` — Layer-2 agent-harness primitive: autonomous loop body.
  Ships the third slice per spec §12: `runtime` module (`Provider`,
  `ToolRegistry`, `RuntimeHandle` traits — the substrate-independent
  seam between ergon and the host runtime; production impls translate
  these to `arcan_provider::Provider` / `praxis_core::ToolRegistry` /
  `arcan_core::TickCtx` in BRO-1001); `step` module (`Step` trait,
  `StepCtx` orchestration arena, `InferenceRequest` builder,
  `DEFAULT_INFERENCE_MAX_TURNS` constant, `run_inference_streaming`
  autonomous loop body that fires every Hook event per spec §5,
  sequential tool dispatch, max-turn budget enforcement). Three more
  spec deviations documented in `crates/ergon/ergon/CLAUDE.md`:
  Provider/ToolRegistry are ergon-owned (not arcan_provider /
  praxis_core direct deps); StepCtx fields trimmed (`journal`,
  `homeostasis`, `soul`, `skills`, `sandbox` cut — moved to auto-hook
  construction or `ToolRegistry` impl internals); `RuntimeHandle`
  narrowed to just `operating_mode()` for v0.1. Result: ergon compiles
  and passes 14 new loop-body unit tests (mocked Provider, ToolRegistry,
  RuntimeHandle, plus BufferSink) with **zero substrate dependencies**.
  43 → 59 tests. (BRO-998)
- `ergon` — Layer-2 agent-harness primitive: hook lifecycle + wire types.
  Ships the second slice per spec §12: `model` module (provider-agnostic
  wire types — `Message` with block-structured content, `ContentBlock`
  enum with Text/Reasoning/ToolUse/ToolResult variants, `ToolCall`,
  `ToolResult`, `ToolDefinition`, `ModelRequest`, `ModelResponse`,
  `Usage`); `hook` module (8-event `Hook` trait, `HookCtx`,
  `HookRegistry`, `HookOutcome` / `ToolHookOutcome` / `InferenceHookOutcome`).
  Hooks observe / deny / stub at workflow / step / inference / tool seams.
  Ergon owns these wire types so the hook contract is decoupled from any
  specific provider crate; `step.rs` (BRO-998) translates between ergon's
  shapes and `arcan_provider` / `praxis_core`. Spec deviation: all 8
  events default to `Ok(_::Continue)` (spec §3.7 only defaulted
  `on_workflow_start`); rationale documented in
  `crates/ergon/ergon/CLAUDE.md`. 18 new unit tests; 43 total. (BRO-997)
- `ergon` — new Layer-2 agent-harness primitive crate. First slice ships
  the foundational trait surface with no Life-substrate dependencies:
  `ErgonError` + `Result` (`error`), `Role` + `RoleScope` with
  call > session > agent precedence (`role`), and `StreamEvent` canonical
  taxonomy + `StreamSink` trait + `BufferSink` + `FanoutSink` (`stream`).
  25 unit tests pass. Spec at
  `docs/superpowers/specs/2026-05-05-ergon-v0.1.md`. Tracker BRO-994.
  Note: spec lists `license = "Apache-2.0"`; the crate uses
  `license.workspace = true` (MIT) for monorepo coherence — life is
  MIT-licensed throughout. (BRO-995, BRO-996)
- `arcan-prosopon` — Pneuma<L0ToExternal> bridge. Subscribes to KernelRuntime
  events, translates to ProsoponEvents, publishes to prosopon-daemon fanout.
  Opt-in via `cargo run -p arcan --features prosopon --prosopon-port <addr>`.
  (BRO-773)

## 0.2.0

- 1077/1077 tests passing, 37 crates, ~43K LOC
- Arcan agent runtime v0.2.1 with full agent loop
- Lago event-sourced persistence v0.2.1
- Autonomic homeostasis controller v0.1.0
- Haima agentic finance engine v0.1.0
- Praxis tool execution sandbox with MCP bridge
- Spaces distributed networking (SpacetimeDB 2.0)
- Vigil OpenTelemetry observability foundation
- Rust 2024 Edition (MSRV 1.85) across all projects
