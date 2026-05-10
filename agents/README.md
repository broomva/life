# Authored Agents

This directory holds the **blessed tier** of authored agents — the
Layer-3 data files (per `docs/superpowers/specs/2026-05-09-bro-1006-authored-agents-architecture.md`)
that ship with the `life` repo. Each `<name>.md` file is parsed by
`ergon::FsAgentRegistry::load()` at runtime startup into an
`AgentSpec`, which is then invocable by name from any agent's loop via
the synthetic `spawn_agent(name, input)` builtin tool.

## Authoring format

```markdown
---
name: <stable-agent-name>          # MUST match the filename stem
model: claude-sonnet-4-5-20250929  # provider-specific model id
max_turns: 16                      # autonomous-loop turn budget
max_retries: 3                     # corrective retries on schema-violation
allowed_tools: null                # null = inherit workflow's full registry; or e.g. [read_file, web_search]
input_schema:                      # JSON Schema (any of draft-7 / 2019-09 / 2020-12)
  type: object
  properties: { ... }
  required: [...]
output_schema:                     # JSON Schema for the typed answer
  type: object
  properties: { ... }
  required: [...]
extensions: {}                     # forward-compat slot; leave empty unless you know what you're doing
---

# Heading (free)

Body text. This becomes the agent's `instructions` field — its
behavioral contract, written as a system prompt.
```

The format is Markdown with YAML frontmatter (the convention every
modern agent ecosystem has converged on: Claude Code skills,
Anthropic prompt library, Vercel AI SDK prompt files, etc.). JSON is
reserved for the **internal wire format** (lago events, network
transports). Markdown is the authoring surface.

## Filename / name match

The `FsAgentRegistry` enforces `name` (in frontmatter) == filename
stem at load time. If they don't match, the registry rejects the
load with a structured `RegistryError::NameMismatch` pointing at the
offending file. This keeps agent identity canonical: there's one
name and one place to find it.

## Currently shipped (BRO-1010)

| Agent | Purpose | Shape |
|-------|---------|-------|
| [`general.md`](general.md) | General-purpose agent for free-form requests when no specialized agent fits. | Multi-turn (max 16), inherits all workflow tools. |
| [`goal-pursuer.md`](goal-pursuer.md) | Multi-turn goal pursuer — takes a goal + success criteria, plans, executes tools, reports structured progress. | Multi-turn (max 32), inherits all workflow tools, uses `spawn_agent` for sub-tasks. |
| [`goal-judge.md`](goal-judge.md) | Single-shot judge that scores a `goal-pursuer`'s output against the original criteria. Designed to run after a pursuer to enforce honesty. | Single-shot (max_turns 1), no tools needed (judging from text alone). |

## Currently shipped (BRO-1011 — meta-agents)

The two meta-agents that watch the bookkeeping pipeline. Both are
**human-PR-authored only** per architecture spec §7.3 — there is no
production code path that lets them edit themselves or each other,
which prevents the metacognition deadlock (a meta-agent rewriting
itself into a corrupt state).

| Agent | Purpose | Shape |
|-------|---------|-------|
| [`nous-promoter.md`](nous-promoter.md) | Reads recent scoring runs from lago, decides what to promote/demote/refresh/synthesize next at the graph level. Allowed tools: `lago_query`, `nous_aggregate`, `nous_compare`. | Multi-turn (max 12), claude-sonnet-4-5. |
| [`nous-judge.md`](nous-judge.md) | Calibration meta-judge for the bookkeeping scorers. Reads a sample of one scorer's runs and verdicts the scorer (`calibrated` / `drifting` / `miscalibrated` / `insufficient_signal`). Suggests prompt edits — never applies them. | Multi-turn (max 4), claude-sonnet-4-5. |

The promoter operates at the **graph level** (entities, slugs, syntheses);
the judge operates at the **scorer level** (the prompts that produce
the scores). They compose: judge → fixes scorer prompts → better
scores → promoter sees better signal → cleaner graph.

## Currently shipped (BRO-1012 — bookkeeping)

The Nous gate (per `skills/bookkeeping/references/scoring-rubric.md`) is
a three-dimension scoring system that decides whether a knowledge item
is promoted from Layer 2 (raw extract) to Layer 3 (entity page). Each
dimension is scored independently by its own single-shot agent. A
fourth agent synthesizes Layer-4 notes from ≥ 3 Layer-3 entities.

| Agent | Purpose | Shape |
|-------|---------|-------|
| [`bookkeeping-novelty.md`](bookkeeping-novelty.md) | Score the Novelty dimension (0–3) against the existing entity graph. | Single-shot (max 1), no tools, claude-haiku-4-5. |
| [`bookkeeping-specificity.md`](bookkeeping-specificity.md) | Score the Specificity dimension (0–3) against the item's concrete grounding. | Single-shot (max 1), no tools, claude-haiku-4-5. |
| [`bookkeeping-relevance.md`](bookkeeping-relevance.md) | Score the Relevance dimension (0–3) against active projects + open questions. | Single-shot (max 1), no tools, claude-haiku-4-5. |
| [`bookkeeping-synthesizer.md`](bookkeeping-synthesizer.md) | Layer-4 synthesizer — combines ≥ 3 entity pages around a topic into a structured synthesis with `[[type/slug]]` citations. Self-flags `blog_post_candidate`. | Multi-turn (max 8), no tools, claude-sonnet-4-5. |

The three scorers share an output shape (`{score, reasoning, anti_pattern_warnings, …}`) so the bookkeeping pipeline (P8) can fan out the same item across all three in parallel and aggregate the raw score. The meta-agents above (BRO-1011) consume these scorers' outputs to detect drift and surface graph-level promotion decisions.

## Self-validation

When you change an agent in this directory:

1. **The file must still parse.** `ergon::parse_agent_md` is run at
   load time; structural errors fail the registry construction.
2. **Filename must match `name` field.** Same check.
3. **Schemas must compile.** The `output_schema` is fed into the
   `jsonschema` crate at the start of every `record_answer`
   validation. A malformed schema is caught at first use.
4. **Run the fixture test:**
   ```bash
   cargo test -p arcan-ergon --test agents_fixtures
   ```
   This loads the `agents/` directory and verifies all expected
   agents are present with the expected names.

## Adding a new agent

1. Create `agents/<your-name>.md` following the format above.
2. Add a row to the table in this README documenting purpose +
   shape.
3. Update the fixture test (`crates/arcan/arcan-ergon/tests/agents_fixtures.rs`)
   if the new agent is a "blessed tier" agent the project depends on.
4. Verify: `cargo test --workspace`
5. Open a PR. Authored agents are **human-PR-authored only** — there
   is no self-modification path in production (per architecture spec
   §7.3, this is the metacognition-deadlock prevention rule).

## Where agents are loaded from

The `arcan` daemon, on startup, calls
`ergon::FsAgentRegistry::load(<--agents-dir>)`. Default value of
`--agents-dir` is `./agents/` relative to the binary's CWD. If the
directory does not exist, arcan falls back to an empty
`InMemoryAgentRegistry` and logs a warning. This means workflows can
invoke `spawn_agent("general", ...)` and the agent runs with the
behavior defined here.

## Future tiers

**Experimental tier** (deferred, per spec §7.3): agents persisted as
lago `Custom("agent.spec")` events. Used for short-lived agents
generated by other agents at runtime. Not in scope for BRO-1010 —
arrives with the future `LagoAgentRegistry` impl.

**Meta-agents** (BRO-1011+): `agents/nous-promoter.md`,
`agents/nous-judge.md`, etc. — themselves authored here, NOT
self-modifying in production.
