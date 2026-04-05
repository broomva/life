# Observability

## Goal

Make Spaces module behavior, reducer performance, and agent development workflows diagnosable without reproducing locally. All critical paths should be traceable via SpacetimeDB server logs and harness CI output.

## Required Event Fields

- `timestamp` — SpacetimeDB-provided ctx.timestamp for reducer events, wall clock for CI
- `level` — log level (trace/debug/info/warn/error)
- `event_name` — reducer name or harness step identifier
- `trace_id` — SpacetimeDB internal trace (available via `spacetime logs`)
- `run_id` — CI run identifier (GitHub Actions run_id)
- `step_id` — harness step (smoke/check/test/lint/typecheck)
- `component` — module layer (auth, users, servers, channels, threads, messages, client)
- `status` — success/failure/error
- `duration_ms` — execution time where measurable

## Event Taxonomy

### Harness Events
- `harness.smoke.start` / `harness.smoke.finish` / `harness.smoke.fail`
- `harness.test.start` / `harness.test.finish` / `harness.test.fail`
- `harness.lint.start` / `harness.lint.finish` / `harness.lint.fail`
- `harness.audit.pass` / `harness.audit.fail`

### SpacetimeDB Module Events
- `module.init` — Spaces Hub created with default channels
- `module.client_connected` — identity connected, profile upserted
- `module.client_disconnected` — presence set offline
- `reducer.success` — reducer completed without error
- `reducer.error` — reducer returned Err (logged by SpacetimeDB)
- `reducer.auth_denied` — authorization check failed (role insufficient)

### Client Events
- `client.connect` — WebSocket connection established
- `client.subscribe` — subscription applied, tables synced
- `client.command` — user issued CLI command
- `client.reducer_call` — reducer invocation sent

## Logging Rules

- Module uses `log::info!` / `log::warn!` / `log::error!` — visible via `spacetime logs spaces`
- Include identity (truncated hex) and affected entity IDs in log messages
- Never log message content or user PII in production
- Harness scripts emit step names and pass/fail status to stdout
- CI workflow captures all stdout/stderr as GitHub Actions artifacts

## Metrics

- WASM module compile time (smoke step 1)
- CLI client compile time (smoke step 2)
- Module publish duration
- Reducer error rate (from `spacetime logs` error count)
- Harness audit pass rate (21 harness checks + 15 control checks)
- CI pipeline total duration

## Alerting

- Alert on WASM compile failure (schema regression)
- Alert on module publish failure (maincloud connectivity or schema conflict)
- Alert on harness/control audit failures in CI
- Alert on elevated reducer error rate (>5% of calls returning Err)
- Alert on client subscription failure (binding mismatch after schema change)
