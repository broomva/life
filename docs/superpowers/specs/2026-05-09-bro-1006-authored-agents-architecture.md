# Spec — Authored Agents as Data: Architecture for Recursive Agent Composition

> **Status**: design committed, substrate work pending
> **BRO ticket**: BRO-1006 (umbrella for the substrate work that lands authored-agents)
> **Date**: 2026-05-09
> **Predecessors**:
> - BRO-1005 (`ergon::Agent` / `AgentSpec` / `TypedAgent` primitive — PR #1198, the language)
> - BRO-1001 (`arcan-ergon` — kernel-side workflow tick adapter, PR #1192)
> - BRO-994 (ergon umbrella)
>
> **Successors** (PRs that land this spec):
> - BRO-1007 (registry + spawn + recursion-context substrate)
> - BRO-1008 (agent CLI tooling — `arcan agent new/list/show/test`)
> - BRO-1009 (nous lineage primitives + nous-tools)
> - BRO-1010 (first authored agents: general, goal-pursuer, goal-judge)
> - BRO-1011 (nous active layer — promoter, NOT improver yet)
> - BRO-1012 (bookkeeping authored agents)

## §0 Abstract

This spec establishes the architectural commitment to **author behavior as
data, not crates**, beyond the universal primitive surface. With the
`Agent` / `AgentSpec` / `TypedAgent` primitive shipped in BRO-1005, the
substrate is in place for compounds (goal-directed pursuit, multi-judge
panels, agent-improvement loops, etc.) to live as **persistent, agent-
authorable, version-tracked data files** rather than Rust crates.

The spec articulates:

1. **Why** data-driven authoring beats crate-based compounds for an
   agentic OS (the "Life as Lisp" thesis).
2. **What** authoring format we commit to — Markdown with YAML
   frontmatter — and why it beats every alternative.
3. **What** failure modes we identified in a premortem, and the
   non-negotiable hardenings the substrate must ship to mitigate them.
4. **What** the three-layer model is (primitives in crates → substrate
   in crates → behaviors as data) and where the line falls.
5. **How** the existing nous metacognition substrate evolves from
   passive scoring hook to active metacognitive agent layer.
6. **Concrete sub-PR breakdown** with hardening commitments per PR.

## §1 The thesis: Life as Lisp

In Lisp, code is data is code. Functions can be defined at runtime,
modified, stored, retrieved, evaluated. The system grows through
self-modification because program text and data text are the same
kind of thing.

The agentic-OS analog is exact:

| Lisp | Life |
|---|---|
| s-expression | `AgentSpec` |
| `eval` / `apply` | `Agent::run` / `run_spec` |
| symbol table | `AgentRegistry` |
| function definition file | `agents/<name>.md` (filesystem) or lago `Custom("agent.spec")` event |
| `funcall` / `apply` | `spawn_agent` builtin tool |
| version-controlled .lisp files | lago event journal (immutable, time-travelable) |

We **already have** the data primitive (`AgentSpec` is serializable,
`JsonSchema`-derivable, returnable as another agent's typed Output,
embeddable in any wire/event/store). What we don't yet have is the
**wiring** that makes it usable as a runtime substrate:

1. **A registry** that maps names → specs, populated from persistent storage
2. **A dispatch primitive** (`spawn_agent`) so agents can invoke specs
   by name from within their loop
3. **A persistence convention** (where authored specs live, how they're
   versioned)
4. **Recursion safety** (depth limits, budget propagation, cycle detection)
5. **Production-grade schema validation** (not hand-rolled)

This spec commits to building all five.

## §2 Why crates are wrong for compounds (and right for primitives)

### Crates are wrong for compounds

Crates are frozen at build time. To change a goal-directed-workflow's
behavior — its prompt, its judge's rubric, its turn budget — a crate-
based compound requires:

1. Edit Rust source
2. Compile the workspace
3. Deploy
4. Restart agents

That's the wrong loop for an agentic OS. An agent that observes its
goal-pursuit failing should be able to **rewrite its own goal-pursuer's
prompt and try again** — same session, same model, new behavior. With
crate-based compounds, the agent can't touch its own behavior. With
data-driven compounds, it can.

Plus: crates can't be created at runtime in Rust. Even if an agent
generates "the perfect Rust implementation of goal-pursuit," the system
can't compile and load it. Specs are immediately runnable — the
interpreter (`run_spec`) executes them without any compilation step.

### Crates are right for primitives

The trait surface (`Agent`, `Workflow`, `Step`, `run_spec`,
`run_inference_streaming`) is the **language agents speak in**. It
needs:

- Compile-time type safety
- Performance (every agent invocation goes through this code)
- Stability (changes break every agent at once)
- Auditability (the whole substrate is in one place)

These are crate properties. The trait surface should be small and
stable; everything else is built on top **as data**.

## §3 The three-layer model

```
┌────────────────────────────────────────────────────────────────────┐
│ Layer 3: AUTHORED PATTERNS                                         │
│   Data. Persisted. Agent-writable. Versioned via lago.             │
│   • agents/goal-pursuer.md                                         │
│   • agents/goal-judge.md                                           │
│   • agents/bookkeeping-scorer.md                                   │
│   • agents/nous-promoter.md (the meta-cognitive agent)             │
│   • agents/agent-improver.md (eventually — see §7)                 │
│   These are AgentSpec values, expressed as MD+frontmatter.         │
├────────────────────────────────────────────────────────────────────┤
│ Layer 2: SUBSTRATE                                                 │
│   Compiled. The wiring that makes Layer 3 usable.                  │
│   • AgentRegistry (in-memory + filesystem-backed + lago-backed)    │
│   • spawn_agent builtin tool                                       │
│   • RecursionContext (depth + budget + cycle detection)            │
│   • Production-grade JSON Schema validation                        │
│   • improve_agent / fork_agent / lago_query tools (eventually)     │
│   • CLI: arcan agent new/list/show/test                            │
├────────────────────────────────────────────────────────────────────┤
│ Layer 1: PRIMITIVES                                                │
│   Compiled. The language.                                          │
│   • Agent / AgentSpec / TypedAgent traits + interpreter (BRO-1005) │
│   • Workflow / Step / StepCtx (BRO-996..999)                       │
│   • run_inference_streaming + autonomous loop (BRO-998)            │
│   • Hook / Stream / Provider / ToolRegistry traits                 │
│   • arcan-ergon kernel-side adapter (BRO-1001)                     │
│   • ergon-life-hooks / ergon-life-sinks (BRO-1000 / 999b)          │
└────────────────────────────────────────────────────────────────────┘
```

Layer 1 is small and stable. Layer 2 is small and (mostly) stable. Layer
3 is large, evolvable, and **agent-authorable**.

### Decision rule: where does a new behavior go?

| Property of the behavior | Layer |
|---|---|
| Universal across all agents (e.g., agent loop, hook firing, stream events) | Layer 1 — Rust crate |
| Substrate plumbing (registry, spawn, depth tracking) | Layer 2 — Rust crate |
| Stable + hot-path + performance-critical (every-tick) | Layer 1.5 — `TypedAgent` impl in a Rust crate |
| Domain-specific OR evolving OR experimental | **Layer 3 — AgentSpec in `agents/<name>.md`** |
| Self-modifying / agent-authored | **Layer 3 — AgentSpec in lago experimental tier, promotable to filesystem via PR** |

**Rule of thumb**: if the behavior is a *prompt* (instructions, rubric,
schema), it's data. If it's a *mechanism* (loop driver, dispatch
plumbing, type system), it's code.

### Migration paths between layers

- **Data → Code**: when an authored AgentSpec stabilizes (used 1000s of
  times, prompt unchanged for weeks, performance is the bottleneck),
  it can be promoted to a `TypedAgent` impl. One-command migration
  via `arcan agent promote-to-rust <name>` (future tooling).
- **Code → Data**: when a `TypedAgent` impl needs frequent prompt
  iteration, it can be ejected to an AgentSpec. Same command, reverse
  direction. The `TypedAgent` impl auto-derives an `AgentSpec`
  already (BRO-1005), so this is mostly a copy-and-relocate.

## §4 Authoring format: Markdown with YAML frontmatter

### Why this format

After comparing JSON, YAML, TOML, Markdown+frontmatter, Rust source,
sexpr, Python/JS, and a custom DSL across criteria that matter for an
agentic OS:

| Criterion | Why it matters | Winner |
|---|---|---|
| LLM emission reliability | Agents WILL author these | Markdown+frontmatter (best multi-line emission) |
| Multiline string ergonomics | `instructions` is the bulk of every spec | Markdown+frontmatter (it IS prose) |
| Comments / inline rationale | Self-modification needs "why we wrote this" | Markdown+frontmatter (native) |
| Diffability for PR review | Promotion gate is human PR review | Markdown+frontmatter (text diff) |
| Schema validation tooling | Validate-on-load must be cheap and strict | Markdown+frontmatter (frontmatter validates as YAML/JSON) |
| Embedded examples / code blocks | Few-shot examples in instructions | Markdown+frontmatter (native) |
| Existing ecosystem precedent | What does Claude Code / Anthropic / Vercel do? | Markdown+frontmatter (Claude Code skills, Anthropic prompts, AI SDK) |

The pattern is convergent across modern agent ecosystems for a reason:
the format matches the shape of the data.

### The format

```markdown
---
name: bookkeeping.score-extract
model: claude-haiku-4-5
max_turns: 1
max_retries: 3
allowed_tools: []
input_schema:
  type: object
  properties:
    text: { type: string }
    source_path: { type: string }
  required: [text]
output_schema:
  type: object
  properties:
    novelty: { type: integer, minimum: 0, maximum: 3 }
    specificity: { type: integer, minimum: 0, maximum: 3 }
    relevance: { type: integer, minimum: 0, maximum: 3 }
  required: [novelty, specificity, relevance]
---

# Score the extract

You score raw research extracts on three axes for the Nous promotion gate.

## Rubric

**Novelty (0-3)**: ...
**Specificity (0-3)**: ...
**Relevance (0-3)**: ...

## Examples

Example 1 — high signal:
\```
Input: { "text": "..." }
Output: { "novelty": 3, "specificity": 3, "relevance": 3 }
\```

## Edge cases

- If the extract is empty: ...
- If the extract is in a non-English language: ...
```

**Frontmatter** is the structured `AgentSpec` data. **Body** is the
`instructions` field — multiline, examples-rich, organized with headers.

### Wire format vs authoring format

- **Authoring format (file system)**: Markdown + YAML frontmatter at
  `agents/<name>.md`
- **Wire format (in-process / lago / network)**: JSON via `serde_json`

A `parse_agent_md(path) -> AgentSpec` function bridges them. Lago events
carry the JSON form. Network transports use JSON. Filesystem and human
authoring use Markdown. No format wars — each context uses the right form.

### LLM emission considerations

Markdown with YAML frontmatter is the **most reliably emittable** structured
format for current LLMs. Empirical evidence: Claude Code skills, Anthropic
prompt files, Vercel AI prompts, every modern agent ecosystem. JSON is also
well-supported but the multi-line string escaping (long `instructions` with
embedded code blocks and quotes) trips models far more than markdown body
text does.

## §5 Premortem: failure modes and hardenings

We ran a premortem against the worst plausible outcome ("six months in,
the approach has failed; engineers want to rip it out"). The full
analysis lives in conversation history; the catalog below is the
distilled output.

### Severe failures (would invalidate the approach if unmitigated)

| # | Failure | Mitigation (NON-NEGOTIABLE — ships with substrate) |
|---|---|---|
| S1 | Agent recursion explodes (A spawns B spawns A → infinite loop, or 50-deep stack, or runaway token bill) | `RecursionContext` with **depth limit** (default 8) + **budget propagation** (token / wall-clock / spawn-count) + **cycle detection** (track spec names in invocation stack). Ships with the spawn_agent PR. Not deferred. |
| S2 | Performance regression (workflows do 10 spawn_agent calls; latency 10x; throughput tanks) | **Hybrid commitment**: `TypedAgent` (compiled) for hot paths; `AgentSpec` (data) for evolving / dynamic. Decision tree documented (§3). |
| S3 | Bad spec promoted (nous-promoter's threshold too lax → buggy spec ships → corrupts data → no rollback) | Lago is append-only → full lineage. **Promotion to "blessed" tier requires PR review** (human gate). nous-promoter only writes to "experimental" tier. |
| S4 | Format wars (half team writes `.md`, half writes `.json`, tooling fragments) | **Pick MD+frontmatter, document it once, enforce it**. Provide CLI: `arcan agent new <name>` scaffolds correct format. Reject non-conforming files at registry load. |

### Moderate failures (require painful course correction)

| # | Failure | Mitigation |
|---|---|---|
| M5 | Schema validation hell (specs pass hand-rolled validator but break runtime in cryptic ways) | Use the [`jsonschema`](https://crates.io/crates/jsonschema) crate (full draft-07/2020-12 validator), not hand-rolled. Validate on load AND on agent emission. Fast-fail with structured errors. |
| M6 | Spec proliferation (200 agents in registry, 60% abandoned experiments) | Same lifecycle as bstack P8 knowledge: scored, gated, retired. nous-promoter retires unused specs after N days idle. Periodic janitor (P9-style). |
| M7 | Prompt fragility / silent regression (new model version breaks an agent; no tests catch it; data corrupts for a week) | Each authored agent has a fixture test (`agents/<name>.test.md` or `.test.json`). Run on agent load + on model version change. nous-promoter watches score drift; alerts. |
| M8 | Debugging recursion is hell (bug 5 levels deep; stack trace useless; lago event count overwhelming) | Tracing spans hierarchy + a `lago replay --tree <run_id>` command (build alongside substrate). Spans give visual recursion tree in vigil/Tempo. |
| M9 | Schema drift (AgentSpec evolves; old lago specs don't match new struct; replay broken) | `#[non_exhaustive]` (already done in BRO-1005) + only additive changes ever. Migration scripts via lago compaction. The `extensions: HashMap<String, Value>` slot absorbs unknowns. |

### Strategic failures (could undermine adoption even if technically working)

| # | Failure | Mitigation |
|---|---|---|
| S10 | "Where's the source?" confusion (new contributors can't find behavior; half is code, half is data) | Documented decision tree (§3) added to `core/life/CLAUDE.md`. `arcan agent list` CLI (BRO-1008). |
| S11 | Hybrid mismatch (some bookkeeping logic in TypedAgent, some in AgentSpec; inconsistent updates → bugs) | Each domain picks ONE primary form. Cross-overs require explicit decision in spec PR. Documented in agents/README.md. |
| S12 | Metacognition deadlock (nous-improver tries to improve nous-promoter; circular dependency; system can't bootstrap) | Hard rule: meta-agents are themselves bootstrapped via PR. Cannot self-modify in production. Documented in CLAUDE.md (§7 elaborates). |
| S13 | ROI inversion for stable patterns (paying LLM tokens to author what could've been a 50-LOC Rust function) | Promote stable AgentSpecs to TypedAgent in Rust when usage stabilizes. Symmetric to "extract pattern when used 3x." `arcan agent promote-to-rust` CLI. |

## §6 Substrate non-negotiables (what BRO-1007 must ship)

The premortem revealed that **severe failures S1-S4 all share a common
shape**: they're guaranteed within weeks of release if the substrate
ships without specific hardenings. These are not deferrable; they ship
together.

### S1 hardening: `RecursionContext`

```rust
pub struct RecursionContext {
    /// Current recursion depth (0 at top-level workflow).
    pub depth: u32,
    /// Hard cap. Default 8 — exceeded → typed `RecursionError::DepthExceeded`.
    pub max_depth: u32,

    /// Stack of agent spec names invoked from root to current frame.
    /// Used for cycle detection: `spawn_agent("foo")` from a frame
    /// whose stack already contains "foo" → typed
    /// `RecursionError::CycleDetected { stack }`.
    pub invocation_stack: Vec<String>,

    /// Total agent invocations (top-level + all descendants) since
    /// workflow tick started. Shared via `Arc<AtomicU32>`.
    pub total_invocations: Arc<AtomicU32>,
    pub max_invocations: u32,

    /// Token budget propagated from parent. Each spawn deducts.
    pub token_budget_remaining: Arc<AtomicI64>,

    /// Wall-clock budget propagated from parent.
    pub wall_clock_budget_remaining: Arc<AtomicI64>,
}

impl RecursionContext {
    /// Check whether spawning a new sub-agent is allowed. Returns
    /// typed error variants the caller surfaces to the model as a
    /// tool error (in-band feedback), not a panic.
    pub fn check_can_spawn(&self, target_spec_name: &str) -> Result<(), RecursionError> { ... }

    /// Build a child context for a spawned sub-agent. Increments
    /// depth, appends to stack, shares atomics.
    pub fn child(&self, target_spec_name: &str) -> Self { ... }
}
```

`spawn_agent` consults `check_can_spawn` before dispatching. Failures
return as `model_error` tool results so the parent agent sees them
in-band and can adapt.

### S2 hardening: hybrid model documentation

A new `agents/README.md` and an extension to `core/life/CLAUDE.md` that
captures the decision tree from §3. Plus a `lints` directory with
golden patterns showing TypedAgent vs AgentSpec for the same problem.

### S3 hardening: lifecycle policy

`agents/<name>.md` files in the repo are the **blessed tier**.
Promotion to blessed requires a PR. Lago `Custom("agent.spec")` events
are the **experimental tier**. nous-promoter writes ONLY to
experimental; humans (or meta-agents with explicit `agent:promote`
capability — initially nobody) move to blessed.

### S4 hardening: format enforcement

`FsAgentRegistry` rejects files that don't match the `parse_agent_md`
shape on load. `arcan agent new <name>` is the canonical way to create
new specs. `arcan agent test <name>` validates the spec + runs the
fixture test.

### M5 hardening: production schema validation

The current `validate_against_schema` in BRO-1005 is intentionally
minimal (object/array/string/number/boolean/null). The BRO-1007
substrate replaces it with a `jsonschema::JSONSchema` validator that
handles the full draft-07/2020-12 spec — `additionalProperties`,
`oneOf` / `anyOf` / `allOf`, `pattern`, `minimum` / `maximum`,
`enum`, `format`, etc. Validation errors carry structured paths and
reasons.

## §7 Nous: from passive substrate to active metacognitive agent

### Current state

`NousScoreHook` (in `ergon-life-hooks`, BRO-1000) fires
`on_post_inference` per inference call, runs the response through
`nous_core::NousEvaluator`, records a score in lago. It's a passive
metacognition signal generator.

### Target state

`NousScoreHook` continues firing. Plus: an active metacognitive agent
layer that reads the accumulated signal and acts.

```
                    ┌─ inference ─┐
agent loop ─────────┤             ├──→ NousScoreHook ──→ lago events
                    └─ response ──┘                       (per-call score data)
                                                                │
                                            ┌───────────────────┘
                                            ▼ (queries via lago_query tool)
                                  ┌──────────────────────────┐
                                  │  agents/nous-promoter.md │
                                  │  (AgentSpec, NOT crate)  │
                                  │                          │
                                  │  reads:                  │
                                  │  • score history per spec│
                                  │  • spec version lineage  │
                                  │  • observed failures     │
                                  │                          │
                                  │  decides:                │
                                  │  • promote v3 to blessed │
                                  │  • retire v1             │
                                  │  • request improvement   │
                                  └──────────────────────────┘
                                            │
                                            ▼
                                  emits Custom("agent.spec.promotion-recommended")
                                  (humans review the recommendation in PR)
```

### Division of labor

| Concern | Form |
|---|---|
| `NousEvaluator` trait, scoring rubrics, heuristics | Rust crate (`nous-core`) |
| `NousScoreHook` fires per-inference | Rust crate (`ergon-life-hooks`) |
| `NousLineage` (aggregate scores per-spec over time, drift detection) | Rust crate (`nous-core`) — small addition |
| `nous-tools` praxis tools (`nous_aggregate`, `nous_compare`, `lago_query`) | Rust crate (`nous-tools`, new) |
| `agents/nous-promoter.md` (decides promote / retire / request improvement) | **AgentSpec** in `agents/` |
| `agents/nous-judge.md` (response-level judge for goal-pursuit) | **AgentSpec** in `agents/` |
| `agents/agent-improver.md` (refines existing specs) | **Deferred** — see §7.3 |

### §7.1 Bootstrapping order

The metacognition layer has potential circular-dependency risk: a
naive design has `nous-improver` improving `nous-promoter`, both of
which gate each other. Hard rule:

> **Meta-agents (nous-*, agent-improver, agent-promoter) are themselves
> bootstrapped via human-reviewed PR. They cannot self-modify in
> production.**

In practice: `agents/nous-promoter.md` and `agents/nous-judge.md` are
human-written initially. They run for ≥30 days observing the agent
fleet before any consideration of self-modification. Self-modification
of meta-agents requires explicit `agent:meta-modify` capability that
defaults to denied.

### §7.2 What nous-promoter does (and doesn't)

**It does:**
- Read `nous_aggregate(spec_name, window)` to see score trajectories
- Read `nous_compare(spec_a, spec_b, window)` to see version-over-version performance
- Read `lago_query` to see raw event history when needed
- Decide promote / retire / request-improvement
- Emit `Custom("agent.spec.promotion-recommended")` event

**It doesn't:**
- Auto-promote anything (only emits recommendations)
- Modify spec content (that's agent-improver's eventual job)
- Modify itself

### §7.3 agent-improver is deferred (S12 mitigation)

`agents/agent-improver.md` (the agent that takes an existing spec +
failure cases + lineage stats and outputs a refined spec) is **NOT
shipped** in the initial substrate. It's deferred until:

1. ≥5 stable authored agents have been in production ≥30 days
2. We have empirical data on what kinds of failures the system actually sees
3. The promoter has been observed making good recommendations

Until then, **improvements are human-authored PRs**. The agent-improver
is a forward-looking primitive whose design is informed by real
observations, not speculation.

## §8 Sub-PR roadmap

Each sub-PR is small, testable in isolation, and ships with the
hardenings from §5/§6 already in place.

| PR | Scope | Hardenings | Estimate |
|---|---|---|---|
| BRO-1007 | `AgentRegistry` trait, `FsAgentRegistry`, `LagoAgentRegistry`, `spawn_agent` builtin tool, **`RecursionContext` with depth/budget/cycle detection**, **jsonschema validation** | S1, S4, M5 — non-negotiable | ~800 LOC + 25 tests |
| BRO-1008 | Agent CLI: `arcan agent new <name>` scaffolds MD+frontmatter, `arcan agent list`, `arcan agent show <name>`, `arcan agent test <name>`, `lago replay --tree <run_id>` | S10, M8 | ~400 LOC + 10 tests |
| BRO-1009 | nous lineage primitives + `nous-tools`: `NousLineage` trait, `nous_aggregate` / `nous_compare` / `lago_query` praxis tools | (enables nous active layer) | ~300 LOC + 8 tests |
| BRO-1010 | First authored agents: `agents/general.md`, `agents/goal-pursuer.md`, `agents/goal-judge.md` + fixture tests | M7 | data only + ~100 LOC fixture-runner |
| BRO-1011 | nous active layer: `agents/nous-promoter.md` + nous-judge.md (NOT improver yet) | S3, S12 | data only |
| BRO-1012 | Bookkeeping authored agents: `agents/bookkeeping-{novelty,specificity,relevance,synthesizer}.md` + fixtures | (validates the use case) | data only |

Total: 6 PRs, ~1500 LOC of substrate + 12+ authored agent definitions.

## §9 Acceptance criteria for "the architecture is sound"

After BRO-1007 and BRO-1010 land, we declare the architecture validated
if all of the following hold:

1. ✅ A workflow body can `spawn_agent(name="goal-pursuer", input=...)`
   and the goal-pursuer behavior runs with full hook lifecycle
   (capability gate, budget gate, score, attest).
2. ✅ Recursion depth limit and cycle detection both fire under
   adversarial test conditions; failures surface as in-band tool
   errors, not panics.
3. ✅ The same goal-pursuer spec can be modified by editing
   `agents/goal-pursuer.md`, no recompile, behavior changes on next
   workflow tick.
4. ✅ `lago replay --tree <run_id>` shows the recursion tree clearly.
5. ✅ Schema validation rejects malformed specs with structured errors
   pointing to the offending field.
6. ✅ `arcan agent new test-agent` produces a valid scaffold; the new
   spec loads and runs without manual intervention.
7. ✅ `cargo test --workspace` and `cargo clippy --workspace --
   -D warnings` are clean across all sub-PRs.

If any of these fail, the architecture is wrong — not the implementation.
We rip out and redesign.

## §10 What is explicitly out of scope (and why)

These are intentionally NOT addressed in this spec or any of its
sub-PRs:

| Feature | Why deferred |
|---|---|
| Daemon / long-lived agents (`in_process_teammate` in noesis taxonomy) | Different shape from `Agent` (no typed return). Sibling primitive when first use case appears. |
| Remote agent dispatch (`AgentInvoker` / `RemoteInvoker`) | `AgentSpec.remote` field reserved (BRO-1005). Wires through `lifegw` when first use case needs it. |
| Async message passing / mailboxes | Future `ergon-life-mailbox` sibling crate when first async use case appears. |
| `agent-improver` (self-improving meta-agent) | Deferred per §7.3 until ≥5 stable agents have ≥30 days of production data. |
| Workflow DSL (declarative workflows-as-data) | Not needed: agents with high `max_turns` + `spawn_agent` access express dynamic workflow behaviors. Workflow-as-Rust-code stays as the static option. |
| Capability for meta-modification (`agent:meta-modify`) | Defaults to denied; granted only after extensive observation. |

Each of these has a clear "first use case" trigger. None block the
core architecture.

## §11 Decision: ship it

This spec captures the architectural commitment. The substrate work
(BRO-1007 onward) implements it.

The premortem revealed three categories of risk. Severe risks are
mitigated by non-negotiable substrate commitments. Moderate risks are
tooling investments that pay down with use. Strategic risks are
documentation + culture.

The approach is sound. **We commit to authored-agents-as-data.**

---

*Spec authored 2026-05-09 in conversation between operator and agent.
Recorded as the architecture of record before any sub-PR work begins.*
