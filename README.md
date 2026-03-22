# Life

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-1077_passing-green.svg)](#)
[![docs](https://img.shields.io/badge/docs-broomva.tech-purple.svg)](https://docs.broomva.tech/docs/life/overview)

**Agent OS monorepo** -- the core primitives that power the BroomVA agent infrastructure. Building artificial life from computational primitives: cognition, persistence, homeostasis, identity, finance, networking, observability, and evaluation.

## Architecture

```
aiOS (kernel contract -- types, traits, event taxonomy)
  |
  +-- Arcan       (cognition + execution -- agent runtime)
  |     +-- Praxis  (tool execution -- sandbox + skills + MCP)
  |     +-- arcan-lago bridge --> Lago (persistence -- event journal + blob store)
  |     +-- arcan-spaces bridge --> Spaces (networking -- distributed agent communication)
  |
  +-- Autonomic   (homeostasis -- stability regulation)
  +-- Haima       (finance -- x402 payments + per-task revenue)
  +-- Anima       (identity -- soul, beliefs, DID, KYA)
  +-- Nous        (evaluation -- metacognitive quality scoring)
  +-- Vigil       (observability -- OpenTelemetry tracing + GenAI metrics)
```

## Sub-crates

| Crate | Description | Status |
|-------|-------------|--------|
| [**arcan**](arcan/) | Core runtime -- event loop, capability system, agent lifecycle | Active |
| [**lago**](lago/) | Persistence -- event-sourced journal, content-addressed blob store | Active |
| [**aiOS**](aiOS/) | Cognitive layer -- kernel contract, LLM integration, reasoning | Active |
| [**autonomic**](autonomic/) | Homeostasis -- three-pillar self-regulation, hysteresis gates | Active |
| [**haima**](haima/) | Finance -- x402 payments, wallets, per-task billing | Active |
| [**anima**](anima/) | Identity -- soul, beliefs, Ed25519/secp256k1 dual keypair, DID | Active |
| [**nous**](nous/) | Evaluation -- inline heuristics + LLM-as-judge quality scoring | Active |
| [**spaces**](spaces/) | Networking -- SpacetimeDB distributed agent communication | Active |
| [**praxis**](praxis/) | Tool execution -- sandbox, hashline editing, MCP bridge | Active |
| [**vigil**](vigil/) | Observability -- OpenTelemetry tracing, GenAI semantic conventions | Active |

## Quick Start

```bash
# Run the agent runtime
cd arcan && cargo run -p arcan

# Run the persistence daemon
cd lago && cargo run -p lagod

# Run homeostasis controller
cd autonomic && cargo run -p autonomicd

# Run all tests across the monorepo
cargo test --workspace
```

## Build & Test

```bash
cargo fmt                     # Format
cargo clippy --workspace      # Lint
cargo test --workspace        # Run all 1077 tests
cargo build --workspace       # Full build
```

## Documentation

Full documentation: [docs.broomva.tech/docs/life/overview](https://docs.broomva.tech/docs/life/overview)

## License

[MIT](LICENSE)
