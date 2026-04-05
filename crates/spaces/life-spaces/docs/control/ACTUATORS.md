# Actuators

Define the actions agents are allowed to perform to move the Spaces system toward setpoints.

## Actuation Surface

- Code edits to SpacetimeDB module (`spacetimedb/src/` — types, tables, auth, reducers, lib)
- Code edits to CLI client (`src/main.rs`)
- Binding regeneration (`spacetime generate --lang rust --out-dir src/module_bindings --module-path ./spacetimedb`)
- Module publish (`spacetime publish spaces --module-path ./spacetimedb`)
- Database reset (`spacetime publish spaces --clear-database -y` — destructive, requires confirmation)
- Harness script updates (`scripts/harness/`, `scripts/control/`)
- Documentation updates (`docs/`, `PLANS.md`, `METALAYER.md`, `AGENTS.md`)
- CI workflow adjustments (`.github/workflows/`)

## Safety Boundaries

- Protected branches/rules: `main` branch requires passing CI before merge
- Restricted commands: `--clear-database` destroys all data — only with explicit user approval; `--force` push prohibited on main
- Approval-required actions: Schema migrations that add/remove/rename tables or columns, SpacetimeDB SDK version bumps, changes to auth.rs role hierarchy

## Action Catalog

| Action | Preconditions | Postconditions | Rollback |
|---|---|---|---|
| Edit reducer code | Smoke passes on current state | Smoke + check pass after edit | `git revert` |
| Add new table | Schema design reviewed | WASM compiles, bindings regenerated | Remove table, regenerate bindings |
| Regenerate bindings | Module compiles to WASM | `src/module_bindings/` matches module schema | Regenerate from prior module version |
| Publish module | Smoke + check pass | `spacetime logs` shows successful init | Republish prior version |
| Clear database | Explicit user approval | Fresh state, init reducer re-seeds | Not reversible (data lost) |
| Update harness script | Script is executable | `make smoke` / `make test` still pass | Restore prior script |
| Update docs | Content is accurate | Audit checks pass | Restore prior docs |
