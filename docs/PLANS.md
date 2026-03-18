---
tags:
  - broomva
  - life
  - roadmap
type: planning
status: active
area: system
created: 2026-03-17
---

# PLANS.md

## Completed Plan A: Docs Traversal + Canonicality (Harness Alignment)

### Objective

- **Outcome**: Agents can quickly understand `/live` structure, locate canonical state, and navigate to detailed docs without ambiguity.
- **Scope**: Root-level doc traversal, canonicality rules, and synchronization guidance.
- **Non-goals**: Rewriting all project-local docs (`arcan/docs`, `lago/docs`, `aiOS/docs`) in this pass.

### Constraints

- **Technical**: Keep docs compact and command-first; avoid duplicate long-form status across files.
- **Policy**: Preserve existing repo conventions and avoid destructive rewrites.
- **Risk**: Documentation drift between root `docs/STATUS.md` and project-local status docs.

### Steps

1. Add a root traversal entrypoint (`docs/NAVIGATION.md`) with read order + precedence.
2. Update `AGENTS.md` docs section to reference navigation and canonicality rules.
3. Mark `docs/STATUS.md` explicitly canonical for `/live` implementation state.
4. Keep follow-up item open for deeper project-local reconciliation.

### Verification

- `test -f docs/NAVIGATION.md`
- `grep -n "Read order for agents" AGENTS.md`
- `grep -n "canonical implementation-state" docs/STATUS.md`

### Decisions

- 2026-02-21 — Root `docs/STATUS.md` is canonical for `/live`; project-local status docs are detailed but subordinate when conflicting.
- 2026-02-21 — Added `docs/NAVIGATION.md` as mandatory first-stop for agents.
- 2026-02-21 — Deferred full cross-repo doc reconciliation to a dedicated cleanup pass.

## Active Plan B: Economic Actuation Track (Conway-Compatible)

### Objective

- **Outcome**: Define how Conway-style ideas integrate into aiOS/Arcan/Lago without breaking contract, replay, or governance guarantees.
- **Scope**: Planned feature documentation and roadmap anchoring only (no runtime implementation yet).
- **Non-goals**: Shipping Conway integration code in this pass.

### Constraints

- **Technical**: Contract-first; no direct vendor lock-in at kernel boundary.
- **Policy**: No paid side effects outside policy + approval paths.
- **Risk**: Economic actions without replay-grade provenance or budget limits.

### Steps

1. Create planned feature spec in `docs/FEATURE_CONWAY_ACTUATION.md`.
2. Add navigation link and question mapping in `docs/NAVIGATION.md`.
3. Anchor track in `docs/ROADMAP.md` as planned cross-cutting feature.
4. Surface in `AGENTS.md` docs index.

### Verification

- `test -f docs/FEATURE_CONWAY_ACTUATION.md`
- `grep -n "Conway" docs/NAVIGATION.md`
- `grep -n "Economic Actuation" docs/ROADMAP.md`
- `grep -n "FEATURE_CONWAY_ACTUATION" AGENTS.md`

### Decisions

- 2026-02-21 — Economic actuation is modeled as a provider-agnostic core feature with Conway as first adapter.
- 2026-02-21 — Payment/resource lifecycle events are required for auditability and replay.
- 2026-02-21 — Budget-aware policy and approval gates are mandatory before write access expansion.