# PLANS.md

## Active Plan: Docs Traversal + Canonicality (Harness Alignment)

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
