# Vigil

Observability foundation for the Life Agent OS — OpenTelemetry-native tracing, GenAI semantic conventions, and contract-derived instrumentation.

**Version**: 0.1.0 | **Rust**: edition 2024, MSRV 1.85 | **Tests**: 26 passing (+2 ignored)

## Architecture

Vigil is a single crate with four modules:

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
