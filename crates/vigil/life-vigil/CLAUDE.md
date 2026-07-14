# Vigil

Observability foundation for the Life Agent OS — OpenTelemetry-native tracing, GenAI semantic conventions, and contract-derived instrumentation.

**Version**: 0.3.0 | **Rust**: edition 2024, MSRV 1.85 | **Tests**: 80 passing (+2 ignored)

## Architecture

Vigil is a single crate composed of the following modules:

### config (`src/config.rs`)

`VigConfig` — telemetry pipeline configuration with environment variable overrides.

- `VigConfig::for_service("arcan")` — create config for a named service
- `VigConfig::from_env()` — build config purely from environment variables
- `config.with_env_overrides()` — apply env overrides on top of programmatic values

### semconv (`src/semconv.rs`)

Semantic convention constants organized into four namespaces:

- **`gen_ai.*`**: GenAI semantic conventions (operation name, system, model, tokens, tool name, agent)
- **`life.*`**: Life Agent OS attributes (session/run/branch IDs, loop phase, operating mode, budget, state vector, tool status)
- **`autonomic.*`**: Autonomic controller attributes (economic mode, health pillars)
- **`lago.*`**: Lago persistence attributes (stream ID, blob hash, fs branch)

### spans (`src/spans.rs`)

Contract-derived span builders that create properly-attributed `tracing` spans:

- `agent_span(session_id, agent_name)` — root `invoke_agent` span for agent sessions
- `phase_span(LoopPhase)` — child span for loop phases (perceive, deliberate, gate, execute, commit, reflect, sleep)
- `chat_span(model, provider, max_tokens, temperature)` — GenAI `chat` client span for LLM calls
- `tool_span(tool_name, tool_call_id)` — GenAI `execute_tool` span for tool calls
- `record_token_usage(span, usage)` — record token counts on a chat span
- `record_finish_reason(span, reason)` — record stop reason
- `write_trace_context(envelope)` — write OTel trace/span IDs into an EventEnvelope (dual-write)
- `extract_trace_context(envelope)` — extract trace context from persisted events

### metrics (`src/metrics.rs`)

`GenAiMetrics` — pre-created OTel metric instruments:

- `gen_ai.client.token.usage` — histogram of token counts per request (input/output breakdown)
- `gen_ai.client.operation.duration` — histogram of LLM call duration (seconds)
- `life.tool.executions` — counter of tool executions by name and status
- `life.budget.tokens_remaining` — gauge of remaining token budget
- `life.budget.cost_remaining_usd` — gauge of remaining cost budget
- `life.mode.transitions` — counter of operating mode transitions

### ledger (`src/ledger.rs`) — intervention ledger (BRO-1880)

Makes a **fork** a first-class recorded Vigil event so a causal claim derived
from a replay is auditable, not testimony. Deterministic replay gives a control
group; a *diagnosis* ("which context fragment caused this write?") needs an
intervention — re-run with exactly one thing changed. That is only honest when
every other variable is pinned; otherwise a fork changes three things while
pretending to change one.

`ForkEvent` carries the **validity tuple**:

- `frozen` — variables pinned to the original run (seeds, scheduler order, tool
  latency/payloads, retrieved context, model version)
- `manipulated` — the single `do()` target
- `free` — variables declared unpinnable for this fork
- `n` — number of fork executions
- `outcomes` — `OutcomeDistribution` over the N runs

Three semantics are enforced at the schema level (machine-checkable, not
asserted):

1. **Single-fork attribution requires total pinning** — `free ≠ ∅ ∧ N == 1` ⇒
   `Attribution::NonAttributive` (a single run cannot average out free variation).
2. **Distributional attribution requires exogeneity** — for `N > 1` the free
   variables must be independent of the manipulated one. The `ExogeneityHook`
   trait (default `PearsonExogeneityHook`) emits an `ExogeneityCheck`
   (`Independent`/`Confounded`/`Indeterminate`) stored on the event; without a
   passing check the distributional claim is non-attributive. Example: latency
   jitter must not correlate with whether the suspect fragment was dropped.
3. **Model-version change is a plant swap, not an intervention** — a `ForkEvent`
   whose manipulated target (or a free variable) is the model version is
   rejected at construction; it must be a `VersionProbeEvent`, which answers
   "is this behaviour version-stable?" via total-variation distance between the
   two versions' outcome distributions — no causal attribution.

**Replayer independence (BRO-1037)** — every event carries both the
`original_runtime` and the `replayer` `RuntimeIdentity`; `replayer_independence()`
flags a `SelfRecorded` ledger (same process instance recording its own forks),
so a fork is `is_valid_evidence()` only when it is *both* attributive *and*
independently recorded. `LedgerEvent` is the tagged (`event_type`) sum of
`Fork`/`VersionProbe` for heterogeneous ledger streams. Span attributes under
`vigil.ledger.*`.

### stream (BRO-1322) — `stream.broadcast.*`

Stream-aware observability lives in the sibling crate `life-stream-metrics`
(dependency-light: `opentelemetry` + `tokio` only, so the substrate primitives
that publish streams can adopt it without the OTLP export machinery). Vigil
re-exports it — `life_vigil::{StreamMetrics, MeasuredSender, MeasuredReceiver,
measured_channel}` — so it is the single observability import surface.

Two surfaces: `StreamMetrics` for call-site instrumentation (where the channel
type must stay a raw `tokio` sender/receiver, or it's an mpsc), and
`MeasuredSender`/`MeasuredReceiver` — a transparent broadcast wrapper for
self-contained channels (instruments both ends + latency in one type swap).

Instrumented channels: `lago.journal.stream` (wrapper), `arcan.substrate`
(call-site), `chronos.wake_router` (call-site, counters only — mpsc). Metrics:
`published_total`, `consumed_total`, `lagged_total`, `skipped_messages_total`,
`buffer_saturation` (gauge 0..1), `consumer_count` (gauge),
`delta_latency_seconds` (histogram). Grafana template at
`dashboards/stream-broadcast.json`.

## Environment Variables

| Variable | Description | Default |
| --- | --- | --- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP collector endpoint (e.g. `http://localhost:4317`) | None (logging only) |
| `OTEL_EXPORTER_OTLP_HEADERS` | Comma-separated `key=value` pairs for OTLP headers | None |
| `OTEL_SERVICE_NAME` | Service identity for OTel resource | `"vigil"` |
| `VIGIL_LOG_FORMAT` | Log output format: `pretty` or `json` | `pretty` |
| `VIGIL_CAPTURE_CONTENT` | Capture prompt/completion content in spans: `true`/`1`/`yes` | `false` |
| `VIGIL_SAMPLING_RATIO` | Trace sampling ratio (0.0..=1.0) | `1.0` |

## Platform Integration Examples

### Langfuse

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://cloud.langfuse.com/api/public/otel"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic <base64(public_key:secret_key)>"
export OTEL_SERVICE_NAME="arcan"
```

### LangSmith

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://api.smith.langchain.com/otel"
export OTEL_EXPORTER_OTLP_HEADERS="x-api-key=<langsmith_api_key>"
export OTEL_SERVICE_NAME="arcan"
```

### Jaeger

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4317"
export OTEL_SERVICE_NAME="arcan"
```

### Grafana Tempo

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4317"
export OTEL_SERVICE_NAME="arcan"
```

## Commands

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test   # Full verify
cargo test                    # Run all tests
cargo test -- --ignored --test-threads=1   # Run env var tests (process-global)
```

## Dependencies

```
aios-protocol (canonical contract — EventEnvelope, LoopPhase, TokenUsage)
  └── vigil (observability — tracing + metrics + GenAI conventions)
```

Vigil depends only on `aios-protocol`. It does NOT depend on Arcan, Lago, Autonomic, Praxis, or Spaces.

## Design Decisions

1. **Graceful degradation**: Without `OTEL_EXPORTER_OTLP_ENDPOINT`, Vigil only configures `tracing-subscriber` for structured logging. No OTel SDK overhead.
2. **Contract-derived spans**: Span hierarchy mirrors the aiOS kernel lifecycle (agent → phase → chat/tool), ensuring 1:1 mapping between observability and runtime behavior.
3. **Dual-write**: `write_trace_context` embeds OTel trace/span IDs into `EventEnvelope`, linking persisted events to their traces for post-hoc analysis.
4. **GenAI semantic conventions**: Follows the OTel GenAI spec (`gen_ai.*` attributes) for compatibility with Langfuse, LangSmith, and other GenAI observability platforms.
5. **`thiserror` for errors**: Library crate convention — `VigError` uses `thiserror` derive.

## Troubleshooting

### "failed to initialize tracing subscriber" error

This happens when `tracing_subscriber::registry().try_init()` is called more than once in the same process. The global subscriber can only be set once. In tests, use `try_init()` (which Vigil does) to tolerate this.

### No spans appearing in Langfuse/LangSmith

1. Check `OTEL_EXPORTER_OTLP_ENDPOINT` is set correctly (include the full URL path)
2. Check `OTEL_EXPORTER_OTLP_HEADERS` has valid auth credentials
3. Ensure the `VigGuard` is kept alive for the application lifetime (it flushes on drop)
