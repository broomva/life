# Life Agent OS

[![CI](https://github.com/broomva/life/actions/workflows/ci.yml/badge.svg)](https://github.com/broomva/life/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-2625_passing-green.svg)](#build--test)
[![Crates](https://img.shields.io/badge/crates-76-blue.svg)](#modules)
[![docs](https://img.shields.io/badge/docs-broomva.tech-purple.svg)](https://broomva.tech/start-here)
[![Contributing](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](CONTRIBUTING.md)

An open-source Rust monorepo for autonomous agents. 13 modules, 76 crates, 2,625 tests.

Life is a contract-first Agent Operating System that treats agents as living systems -- with cognition, persistence, homeostasis, identity, finance, networking, observability, and evaluation as first-class computational primitives. Each maps to a biological analog:

| Primitive | Biological Analog | Module |
|-----------|------------------|--------|
| Cognition + Execution | Central nervous system | **Arcan** |
| Tool Execution | Motor cortex / effectors | **Praxis** |
| Persistence | Long-term memory | **Lago** |
| Homeostasis | Autonomic nervous system | **Autonomic** |
| Finance | Circulatory system | **Haima** |
| Identity | DNA + immune identity | **Anima** |
| Evaluation | Metacognition | **Nous** |
| Networking | Social/swarm behavior | **Spaces** |
| Observability | Proprioception | **Vigil** |
| Contract / Genome | DNA | **aiOS** |

## Repository Structure

```
life/
├── Cargo.toml                    # Unified workspace (76 crates)
├── crates/
│   ├── aios/                     # Kernel contract (7 crates)
│   ├── arcan/                    # Agent runtime (19 crates) -- cargo install arcan
│   ├── lago/                     # Persistence (16 crates) -- lagod daemon
│   ├── autonomic/                # Homeostasis (5 crates) -- autonomicd daemon
│   ├── praxis/                   # Tool execution (4 crates)
│   ├── haima/                    # Finance (8 crates) -- haimad daemon
│   ├── nous/                     # Evaluation (7 crates) -- nousd daemon
│   ├── anima/                    # Identity (3 crates)
│   ├── vigil/                    # Observability (1 crate)
│   ├── spaces/                   # Networking (1 crate + WASM)
│   ├── relay/                    # Remote sessions (3 crates)
│   ├── cli/                      # Life CLI (1 crate)
│   └── spaces-a2a/               # A2A bridge (1 crate)
├── .github/workflows/
│   ├── ci.yml                    # 11-job CI pipeline
│   ├── release.yml               # release-plz automation
│   ├── release-binaries.yml      # Binary distribution
│   └── mirror-sync.yml           # Read-only repo mirrors
└── docs/
```

## Modules

| Module | Crates | Description | Key Binary | Docs |
|--------|--------|-------------|------------|------|
| [**aiOS**](crates/aios/) | 7 | Kernel contract -- canonical types, traits, event taxonomy | -- | [CLAUDE.md](crates/aios/CLAUDE.md) |
| [**Arcan**](crates/arcan/) | 19 | Agent runtime -- event loop, LLM providers, streaming, TUI | `arcan` | [CLAUDE.md](crates/arcan/CLAUDE.md) |
| [**Lago**](crates/lago/) | 16 | Event-sourced persistence -- journal, blob store, knowledge graph | `lagod` | [CLAUDE.md](crates/lago/CLAUDE.md) |
| [**Autonomic**](crates/autonomic/) | 5 | Homeostasis -- three-pillar regulation, hysteresis gates | `autonomicd` | [CLAUDE.md](crates/autonomic/CLAUDE.md) |
| [**Praxis**](crates/praxis/) | 4 | Tool execution -- sandbox, hashline editing, MCP bridge | -- | [CLAUDE.md](crates/praxis/CLAUDE.md) |
| [**Haima**](crates/haima/) | 8 | Finance -- x402 payments, secp256k1 wallets, per-task billing | `haimad` | [CLAUDE.md](crates/haima/CLAUDE.md) |
| [**Nous**](crates/nous/) | 7 | Metacognitive evaluation -- inline heuristics, LLM-as-judge | `nousd` | [CLAUDE.md](crates/nous/CLAUDE.md) |
| [**Anima**](crates/anima/) | 3 | Identity -- soul profiles, belief states, DID | -- | -- |
| [**Vigil**](crates/vigil/) | 1 | Observability -- OpenTelemetry, GenAI semantic conventions | -- | [CLAUDE.md](crates/vigil/life-vigil/CLAUDE.md) |
| [**Spaces**](crates/spaces/) | 1 | Distributed networking -- SpacetimeDB 2.0, RBAC | -- | [CLAUDE.md](crates/spaces/life-spaces/CLAUDE.md) |
| [**Relay**](crates/relay/) | 3 | Remote agent sessions -- WebSocket relay daemon | `relayd` | [CLAUDE.md](crates/relay/CLAUDE.md) |
| [**CLI**](crates/cli/) | 1 | Life CLI -- deployment pipeline | `life-cli` | -- |
| [**Spaces A2A**](crates/spaces-a2a/) | 1 | A2A protocol bridge for Spaces | `spaces-a2a` | -- |
| | **76** | | | |

## Prerequisites

- [**Rust 1.93+**](https://rustup.rs/) (2024 Edition)
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  rustup update   # if already installed
  ```
- **Protobuf compiler** -- required for gRPC codegen (lago, haima)
  ```bash
  # macOS
  brew install protobuf

  # Ubuntu/Debian
  sudo apt-get install -y protobuf-compiler

  # Windows
  choco install protoc
  # or download from https://github.com/protocolbuffers/protobuf/releases
  ```
- **Build time**: ~5-15 min initial compile, ~1 min incremental

## Quick Start

### Zero-config (no API key needed)

```bash
cargo install arcan

# Interactive agent shell with mock provider -- works immediately
arcan shell --provider mock

# Or start the daemon + TUI client
arcan serve --provider mock   # terminal 1
arcan chat                    # terminal 2
```

### With a real LLM provider

```bash
# Anthropic Claude
ANTHROPIC_API_KEY=sk-ant-... arcan shell

# Local Ollama
arcan shell --provider ollama --model gemma4

# Any OpenAI-compatible API
OPENAI_API_KEY=... OPENAI_BASE_URL=https://api.together.xyz arcan shell --provider openai --model meta-llama/Llama-3.3-70B-Instruct-Turbo
```

See [`.env.example`](.env.example) for all configuration options.

### Build from source

```bash
git clone https://github.com/broomva/life.git
cd life
cargo check --workspace       # Verify compilation (76 crates)
cargo test --workspace        # Run 2,625+ tests

# Set up conventional commit hooks
git config core.hooksPath .githooks
git config commit.template .gitmessage
```

### Daemons

```bash
cargo run -p arcan -- serve   # Agent runtime (port 3000)
cargo run -p lagod            # Persistence (port 8080/50051)
cargo run -p autonomicd       # Homeostasis controller (port 3002)
cargo run -p haimad           # Finance engine (port 3003)
```

## Architecture

```mermaid
graph TD
    aiOS["<b>aiOS</b><br/>Kernel Contract<br/><i>types, traits, events</i>"]

    Arcan["<b>Arcan</b><br/>Agent Runtime"]
    Lago["<b>Lago</b><br/>Persistence"]
    Autonomic["<b>Autonomic</b><br/>Homeostasis"]
    Haima["<b>Haima</b><br/>Finance"]
    Anima["<b>Anima</b><br/>Identity"]
    Nous["<b>Nous</b><br/>Evaluation"]
    Praxis["<b>Praxis</b><br/>Tool Execution"]
    Spaces["<b>Spaces</b><br/>Networking"]
    Vigil["<b>Vigil</b><br/>Observability"]

    aiOS --> Arcan
    aiOS --> Lago
    aiOS --> Autonomic
    aiOS --> Haima
    aiOS --> Anima
    aiOS --> Nous
    aiOS --> Praxis
    aiOS --> Spaces
    aiOS --> Vigil

    Arcan --> Praxis
    Arcan --> Lago
    Arcan --> Spaces
    Arcan --> Autonomic
    Nous --> Arcan
    Vigil -.-> Arcan
    Vigil -.-> Lago
    Haima --> Lago
    Anima --> Lago

    style aiOS fill:#4a9eff,stroke:#2d6bc4,color:#fff
    style Arcan fill:#ff6b6b,stroke:#c44d4d,color:#fff
    style Lago fill:#51cf66,stroke:#37a34d,color:#fff
    style Autonomic fill:#ffd43b,stroke:#c4a230,color:#000
    style Haima fill:#cc5de8,stroke:#9b45b3,color:#fff
    style Anima fill:#ff922b,stroke:#c47020,color:#fff
    style Nous fill:#20c997,stroke:#17976e,color:#fff
    style Praxis fill:#748ffc,stroke:#5770c4,color:#fff
    style Spaces fill:#f06595,stroke:#b84d72,color:#fff
    style Vigil fill:#868e96,stroke:#656b71,color:#fff
```

> All modules depend on aiOS (the kernel contract). Modules never import each other's internals -- only bridge crates connect them.

## Build & Test

```bash
cargo fmt --all                          # Format
cargo clippy --workspace -- -D warnings  # Lint (zero warnings policy)
cargo test --workspace                   # 2,625 tests
cargo build --workspace                  # Full build
```

## CI/CD

The monorepo uses a single [CI workflow](.github/workflows/ci.yml) with 11 jobs:

| Job | Check |
|-----|-------|
| Format | `cargo fmt --check` |
| Lint | `cargo clippy -D warnings` |
| Test (Linux) | `cargo test --workspace` |
| Test (macOS) | `cargo test --workspace` |
| MSRV | `cargo check` with Rust 1.93 |
| Security Audit | `rustsec/audit-check` |
| Dependency Check | `cargo deny check` |
| Secret Scan | `trufflehog --only-verified` |
| Commit Lint | Conventional commits on PR titles |
| Console | arcan-console frontend (bun) |
| Build Release | `cargo build --release` |

Releases are automated via [release-plz](https://release-plz.ieni.dev/) with per-crate semver from conventional commits.

## Individual Repo Mirrors

Each module is also available as a read-only mirror for standalone use:

| Mirror | Install |
|--------|---------|
| [broomva/arcan](https://github.com/broomva/arcan) | `cargo install arcan` |
| [broomva/lago](https://github.com/broomva/lago) | -- |
| [broomva/autonomic](https://github.com/broomva/autonomic) | -- |
| [broomva/praxis](https://github.com/broomva/praxis) | -- |
| [broomva/haima](https://github.com/broomva/haima) | -- |
| [broomva/nous](https://github.com/broomva/nous) | -- |
| [broomva/anima](https://github.com/broomva/anima) | -- |
| [broomva/vigil](https://github.com/broomva/vigil) | -- |
| [broomva/spaces](https://github.com/broomva/spaces) | -- |
| [broomva/aiOS](https://github.com/broomva/aiOS) | -- |

> Development happens in this monorepo. Mirrors are synced automatically via [splitsh-lite](https://github.com/splitsh/lite).

## Documentation

- [Quickstart](docs/QUICKSTART.md) -- 30-second start + decision tree
- [Module Guide](docs/MODULE_GUIDE.md) -- All 76 crates categorized by tier
- [Architecture](docs/ARCHITECTURE.md) -- System design
- [Status](docs/STATUS.md) -- Implementation health dashboard
- [Roadmap](docs/ROADMAP.md) -- Development phases
- [broomva.tech/start-here](https://broomva.tech/start-here) -- Getting started guide

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

This project uses [conventional commits](https://www.conventionalcommits.org/). Set up the commit template:

```bash
git config commit.template .gitmessage
git config core.hooksPath .githooks
```

## License

[MIT](LICENSE)
