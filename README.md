# Vigil

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![docs](https://img.shields.io/badge/docs-broomva.tech-purple.svg)](https://docs.broomva.tech/docs/life/vigil)

Observability primitive for the [Agent OS](https://github.com/broomva) -- OpenTelemetry-native tracing, GenAI semantic conventions, and contract-derived instrumentation.

Vigil provides structured logging, distributed tracing, and metrics collection for agent sessions. It follows the OpenTelemetry GenAI semantic conventions for compatibility with Langfuse, LangSmith, Jaeger, Grafana Tempo, and other observability platforms.

## Architecture

Vigil is a single crate with four modules:

| Module | Purpose |
|--------|---------|
| `config` | Telemetry pipeline configuration with environment variable overrides |
| `semconv` | Semantic convention constants (`gen_ai.*`, `life.*`, `autonomic.*`, `lago.*`) |
| `spans` | Contract-derived span builders mirroring the agent lifecycle |
| `metrics` | Pre-created OTel metric instruments for tokens, latency, budget, and mode transitions |

## Key Features

- **Graceful degradation** -- without an OTLP endpoint, Vigil only configures `tracing-subscriber` for structured logging. No OTel SDK overhead.
- **Contract-derived spans** -- span hierarchy mirrors the aiOS kernel lifecycle (agent, phase, chat/tool), ensuring 1:1 mapping between observability and runtime behavior.
- **Dual-write** -- embeds OTel trace/span IDs into `EventEnvelope`, linking persisted events to their traces for post-hoc analysis.
- **GenAI semantic conventions** -- follows the OTel GenAI spec (`gen_ai.*` attributes) for cross-platform compatibility.

## Getting Started

```bash
cargo test                # Run all 26 tests
cargo clippy -- -D warnings   # Lint
cargo fmt                 # Format
```

## Configuration

| Variable | Description | Default |
|----------|-------------|---------|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP collector endpoint | None (logging only) |
| `OTEL_EXPORTER_OTLP_HEADERS` | Comma-separated `key=value` auth headers | None |
| `OTEL_SERVICE_NAME` | Service identity for OTel resource | `"vigil"` |
| `VIGIL_LOG_FORMAT` | Log output format: `pretty` or `json` | `pretty` |
| `VIGIL_CAPTURE_CONTENT` | Capture prompt/completion content in spans | `false` |
| `VIGIL_SAMPLING_RATIO` | Trace sampling ratio (0.0..=1.0) | `1.0` |

## Requirements

- Rust 2024 edition (MSRV 1.85)
- Depends only on `aios-protocol` from the Agent OS stack

## Documentation

Full documentation: [docs.broomva.tech/docs/life/vigil](https://docs.broomva.tech/docs/life/vigil)

## License

[MIT](LICENSE)
