# /live Documentation Navigation (Agent-First)

Last updated: 2026-03-03

This file is the **entrypoint for agents**. It explains where to start, which docs are canonical, and how to traverse to deeper project detail without guessing.

---

## 1) Canonicality Rules (read this first)

When documents disagree, use this precedence:

1. `docs/STATUS.md` (**canonical implementation state for /live**)
2. `docs/ARCHITECTURE.md` (system boundaries + runtime/persistence shape)
3. `docs/CONTRACT.md` (event/schema invariants)
4. Project-local docs for implementation detail:
   - `arcan/docs/*`
   - `lago/docs/*`
   - `aiOS/docs/*`

If any project-local status conflicts with root status, treat root `docs/STATUS.md` as source of truth and log a doc-drift follow-up.

---

## 2) Fast Read Path (5–10 minutes)

Use this path for operational awareness before making changes:

1. `AGENTS.md` (repo constraints, commands, guardrails)
2. `docs/NAVIGATION.md` (this file)
3. `docs/STATUS.md` (what is done / blocked / partial)
4. `docs/ARCHITECTURE.md` (where things belong)
5. `docs/ROADMAP.md` (what should happen next)

---

## 3) Deep Read Path (full understanding)

After fast path, expand by concern:

### Runtime behavior (Arcan)
- `arcan/docs/STATUS.md`
- `arcan/docs/lago-integration.md`
- `arcan/docs/harness.md`

### Persistence + policy (Lago)
- `lago/docs/README.md`
- `lago/docs/architecture.md`
- `lago/docs/policy-engine.md`
- `lago/docs/api-reference.md`

### Kernel contract (aiOS)
- `aiOS/docs/README.md`
- `aiOS/docs/ARCHITECTURE.md`
- `aiOS/docs/REFERENCE.md`
- `aiOS/docs/STATUS.md`

### Distributed networking (Spaces)
- `spaces/CLAUDE.md` (SpacetimeDB rules, common mistakes, hard requirements)
- `spaces/PLANS.md` (execution plan and decision log)
- `spaces/spacetimedb/src/lib.rs` (WASM module entry point)
- `spaces/src/main.rs` (CLI client)

### Testing and quality gates
- `docs/TESTING.md`
- `Makefile.harness`
- `scripts/harness/*.sh`

### Governance / control system
- `docs/control/CONTROL_LOOP.md` (consolidated: loop, sensors, setpoints, actuators, stability, observability)
- `docs/control/ARCHITECTURE.md` (boundaries and ownership)
- `docs/control/OBSERVABILITY.md` (event taxonomy for instrumentation)

### Planned feature tracks
- `docs/FEATURE_CONWAY_ACTUATION.md` (economic actuation plane, Conway-compatible)

---

## 4) Question → Where to look

- “What is implemented right now?” → `docs/STATUS.md`
- “What are the architecture boundaries?” → `docs/ARCHITECTURE.md`
- “What contract/invariants must hold?” → `docs/CONTRACT.md`
- “What should we build next?” → `docs/ROADMAP.md` + `PLANS.md`
- “How does Arcan talk to Lago?” → `arcan/docs/lago-integration.md`
- “How do Conway-style economic ideas map to Agent OS?” → `docs/FEATURE_CONWAY_ACTUATION.md`
- "How does agent-to-agent networking work?" → `spaces/CLAUDE.md`, `spaces/spacetimedb/src/`
- "How do I run safe checks quickly?" → `make smoke`, `make check`, `make ci`
- "What is failing in governance/harness?" → `make audit` + control docs

---

## 5) Structure Map

```text
/live
  AGENTS.md                  # command-first repo operating guide
  PLANS.md                   # active multi-step execution plans
  docs/
    NAVIGATION.md            # this file (agent traversal entrypoint)
    STATUS.md                # canonical current implementation state
    ARCHITECTURE.md          # cross-project architecture + boundaries
    CONTRACT.md              # canonical protocol/schema invariants
    ROADMAP.md               # phase plan and sequencing
    FEATURE_CONWAY_ACTUATION.md  # planned economic actuation track
    TESTING.md               # test strategy and quality expectations
    PLATFORM.md              # OS analogy, crate roles, SaaS trajectory
    arcan.md                 # executive vision and positioning
    control/
      CONTROL_LOOP.md        # consolidated control reference (loop, sensors, setpoints, actuators, stability)
      ARCHITECTURE.md        # control-aware boundaries and ownership
      OBSERVABILITY.md       # required event types and fields
  arcan/docs/*               # runtime-specific deep docs
  lago/docs/*                # persistence/policy deep docs
  aiOS/docs/*                # kernel contract deep docs
  spaces/                    # distributed agent networking (SpacetimeDB)
    CLAUDE.md                # SpacetimeDB rules and patterns
    spacetimedb/src/         # WASM module (server-side)
    src/                     # CLI client
```

---

## 6) Maintenance Rules (Harness-aligned)

Any substantial implementation change should update docs in this order:

1. Update `docs/STATUS.md` (state changed)
2. Update `docs/ARCHITECTURE.md` if boundaries/flow changed
3. Update project-local deep docs (`arcan/docs`, `lago/docs`, `aiOS/docs`) for detail
4. Update `PLANS.md` decisions/checkpoints if this was part of a planned execution

Keep docs compact, actionable, and command-first.
