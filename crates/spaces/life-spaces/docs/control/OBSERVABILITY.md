# Control Observability

## Required Fields

- `run_id` — CI run identifier or local invocation timestamp
- `trace_id` — SpacetimeDB internal trace for reducer calls
- `task_id` — development task identifier (from PLANS.md or issue tracker)
- `command_id` — harness command that was executed (smoke, check, test, lint, typecheck)
- `status` — success / failure / error / skipped
- `duration_ms` — wall-clock time for the step

## Required Events

- `control.smoke.start` — WASM module compile initiated
- `control.smoke.success` — module + client compile both passed
- `control.smoke.failure` — compile failed (includes which step: wasm or client)
- `control.check.start` — clippy linting initiated
- `control.check.success` — no warnings with `-D warnings`
- `control.check.failure` — clippy found issues
- `control.test.start` — full build + optional publish test initiated
- `control.test.success` — all test steps passed
- `control.test.failure` — test step failed (includes which step)
- `control.publish.start` — `spacetime publish` initiated
- `control.publish.success` — module deployed to maincloud
- `control.publish.failure` — publish failed (schema conflict, connectivity, etc.)
- `control.audit.pass` — harness or control audit scored 100%
- `control.audit.failure` — audit found missing files or sections
- `control.escalation` — retry budget exceeded, human review required

## Trace Correlation

- CI runs produce a `run_id` visible in GitHub Actions
- SpacetimeDB logs are queryable via `spacetime logs spaces` and contain reducer-level traces
- Harness scripts output step names and durations to stdout, captured by CI
- To correlate: match CI `run_id` + git commit SHA to `spacetime logs` timestamp window around the publish step
