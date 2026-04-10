---
tags:
  - broomva
  - life
  - observability
  - control
type: operations
status: active
area: observability
created: 2026-03-17
---

# Control Observability

**Last updated**: 2026-03-01

Metrics, events, and diagnostic signals for the control plane.

---

## Required Event Fields

Every control event must include:

| Field | Type | Description |
|-------|------|-------------|
| `timestamp` | ISO 8601 | When the event occurred |
| `run_id` | string | Unique identifier for the CI/harness run |
| `trace_id` | string | Correlation ID across related steps |
| `command_id` | string | Which command was invoked (smoke, check, test, etc.) |
| `task_id` | string | Specific task or gate within the command |
| `component` | string | Which workspace/crate is being checked |
| `status` | enum | `started`, `passed`, `failed`, `skipped` |
| `duration_ms` | integer | Execution time in milliseconds |
| `level` | enum | `info`, `warn`, `error` |

---

## Event Taxonomy

### Gate Events
- `control.gate.start` — gate begins execution
- `control.gate.pass` — gate completes successfully
- `control.gate.fail` — gate fails (includes error context)
- `control.gate.retry` — gate retried after failure
- `control.gate.skip` — gate skipped (precondition not met)

### Audit Events
- `control.audit.start` — audit run begins
- `control.audit.check.pass` — individual audit check passes
- `control.audit.check.fail` — individual audit check fails
- `control.audit.complete` — audit run finishes (includes pass/fail count)

### Escalation Events
- `control.escalation.triggered` — retry budget exhausted, human needed
- `control.escalation.resolved` — human intervention completed

### Recovery Events
- `control.recovery.start` — recovery workflow initiated
- `control.recovery.action` — specific recovery action taken (e.g., `cargo fmt`)
- `control.recovery.result` — recovery outcome (success/partial/failed)

---

## Metrics

### Primary Setpoints

Defined in `evals/control-metrics.yaml` (calibrated 2026-02-28):

| Metric | Target | Alert Threshold | Source |
|--------|--------|-----------------|--------|
| `pass_at_1` | 1.00 | < 0.90 | CI test results |
| `retry_rate` | 0.10 | > 0.30 | CI retry counts |
| `merge_cycle_time` | 24h | > 48h | SCM timestamps |
| `revert_rate` | 0.03 | > 0.08 | SCM revert commits |
| `human_intervention_rate` | 0.15 | > 0.35 | Review/escalation logs |

### Derived Metrics

| Metric | Formula | Purpose |
|--------|---------|---------|
| `time_to_actionable_failure` | First failure event timestamp - run start | How fast failures surface |
| `gate_pass_rate` | Gates passed / gates attempted | Per-gate health |
| `conformance_coverage` | Suites passing / total suites | Behavioral confidence |
| `audit_gap_count` | Failed checks in strict audit | Infrastructure completeness |

---

## Sensors

| Sensor | Sampling | Source Script |
|--------|----------|--------------|
| CI gate results | Every push/PR | `scripts/control/{smoke,check,test}.sh` |
| Test outcomes | Every test run | `cargo test` output parsing |
| Architecture violations | Every audit | `scripts/architecture/verify_dependencies.sh` |
| Conformance results | Every audit | `conformance/run.sh` |
| Control artifact existence | Every audit | `scripts/audit_control.sh` |
| Nightly entropy | Daily 04:00 UTC | `control-nightly.yml` |

---

## Logging Rules

1. **Structured output**: gate scripts emit machine-parseable status lines.
2. **Stable field names**: field names must not change without version bump in `evals/control-metrics.yaml`.
3. **Failure context**: every failure event includes enough context to diagnose without reproduction.
4. **Secret redaction**: never log API keys, tokens, or credentials.
5. **Duration tracking**: every gate and audit step reports `duration_ms`.

---

## Alerting Conditions

| Condition | Response |
|-----------|----------|
| `pass_at_1` drops below 0.90 | Block merges, investigate regressions |
| `retry_rate` exceeds 0.30 | Review flaky tests, check infra stability |
| `merge_cycle_time` exceeds 48h | Review blocking PRs, check CI queue |
| Conformance suite failure | Block deployment, run recovery |
| Architecture audit failure | Block merges, fix dependency violation |
| Strict audit missing files | Create missing artifacts before next gate |

---

## CI Artifacts

| Artifact | Retention | Location |
|----------|-----------|----------|
| `.life/control/state.json` | 30 days | GitHub Actions artifact |
| Gate pass/fail summary | Per-run | `$GITHUB_STEP_SUMMARY` |
| Test output logs | Per-run | CI job output |

---

## Runtime Observability (Vigil)

The `vigil` crate provides runtime observability for the Agent OS using OpenTelemetry.

### Span Hierarchy

```
invoke_agent (session)
  ├── loop_phase (perceive)
  ├── loop_phase (deliberate)
  ├── loop_phase (gate)
  ├── loop_phase (execute)
  │     ├── chat (LLM call — gen_ai.* attributes)
  │     └── execute_tool (tool call — gen_ai.tool.* attributes)
  ├── loop_phase (commit)
  ├── loop_phase (reflect)
  └── loop_phase (sleep)
```

### GenAI Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `gen_ai.client.token.usage` | Histogram | Token counts per request (input/output) |
| `gen_ai.client.operation.duration` | Histogram | LLM call duration (seconds) |
| `vigil.requests` | Counter | LLM requests by provider, model, operation, and status |
| `vigil.estimated_cost_usd` | Counter | Cumulative estimated LLM cost by provider, model, operation, and route |
| `life.tool.executions` | Counter | Tool executions by name and status |
| `life.budget.tokens_remaining` | Gauge | Remaining token budget |
| `life.budget.cost_remaining_usd` | Gauge | Remaining cost budget (USD) |
| `life.mode.transitions` | Counter | Operating mode transitions |

LLM metrics intentionally use only low-cardinality dimensions: `gen_ai.system`, `gen_ai.request.model`, `gen_ai.operation.name`, `vigil.status`, and `vigil.route`. Per-call identifiers remain in spans, JSONL artifacts, and Lago events.

### LLM Reliability Signals

Provider adapters attach reliability observations to the `chat` span and the persisted `vigil.llm_call` envelope:

| Attribute | Source | Notes |
|-----------|--------|-------|
| `vigil.llm.retry_count` | Provider retry loop | Populated for OpenAI-compatible non-streaming calls. |
| `vigil.llm.time_to_first_token_ms` | Streaming provider parser | Populated when a streamed first content/tool delta is observable. |
| `vigil.llm.finish_reason` | Provider response | Preserves the raw provider finish/stop reason. |
| `vigil.llm.fallback_triggered` | Provider/routing telemetry | Defaults to `false` until router fallback is implemented. |
| `vigil.llm.fallback_reason` | Provider/routing telemetry | Optional reason for fallback decisions. |
| `vigil.llm.circuit_state` | Provider/routing telemetry | Defaults to `closed` until circuit-breaker state is implemented. |

These remain span/envelope attributes rather than metric dimensions to avoid high-cardinality cost and reliability metrics.

### Configuration

Set `OTEL_EXPORTER_OTLP_ENDPOINT` to enable OTel export. Without it, Vigil degrades to structured logging only.

See `vigil/CLAUDE.md` for full configuration reference and platform integration examples (Langfuse, LangSmith, Jaeger, Grafana Tempo).
