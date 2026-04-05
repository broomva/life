# Contributing to Life

Thanks for your interest in contributing to Life! This document provides guidelines and information for contributors.

## Getting Started

```bash
git clone https://github.com/broomva/life.git
cd life
cargo test --workspace    # Run all 2,625+ tests
cargo clippy --workspace -- -D warnings  # Zero warnings policy

# Set up commit hooks (conventional commits enforced)
git config core.hooksPath .githooks
git config commit.template .gitmessage
```

### Prerequisites

- **Rust** 2024 Edition (MSRV 1.93) via [rustup](https://rustup.rs/)
- **Protobuf compiler** (`protoc`) for gRPC codegen
- **SpacetimeDB CLI** (optional, only for Spaces development)

## How to Contribute

### Good First Issues

Look for issues labeled [`good first issue`](https://github.com/broomva/life/labels/good%20first%20issue). These are scoped, well-described tasks ideal for getting familiar with the codebase.

### Reporting Bugs

[Open a bug report](https://github.com/broomva/life/issues/new?template=bug_report.yml) with:
- What you expected vs. what happened
- Steps to reproduce
- Rust version (`rustc --version`) and OS

### Suggesting Features

[Open a feature request](https://github.com/broomva/life/issues/new?template=feature_request.yml) with:
- The problem you're trying to solve
- Your proposed approach
- Which subsystem(s) it touches

## Development Workflow

### 1. Fork and Branch

```bash
git checkout -b your-feature-name
```

### 2. Make Changes

Follow existing patterns in the subsystem you're modifying. Each subsystem has its own directory:

| Subsystem | Directory | What it does |
|-----------|-----------|--------------|
| Arcan | `arcan/` | Agent runtime daemon |
| Lago | `lago/` | Event-sourced persistence |
| aiOS | `aiOS/` | Kernel contract (types, traits) |
| Autonomic | `autonomic/` | Homeostasis controller |
| Haima | `haima/` | Agentic finance |
| Anima | `anima/` | Identity and belief |
| Nous | `nous/` | Metacognitive evaluation |
| Praxis | `praxis/` | Tool execution sandbox |
| Spaces | `spaces/` | Distributed networking |
| Vigil | `vigil/` | Observability |

### 3. Test

```bash
# Run tests for a specific subsystem
cargo test -p arcan-core

# Run all tests
cargo test --workspace

# Lint (must pass with zero warnings)
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all
```

### 4. Submit a Pull Request

- Fill out the PR template
- Link any related issues
- Ensure CI passes (clippy, tests, architecture audit)

## Architecture Rules

These are enforced by CI and are non-negotiable:

1. **Subsystems depend on `aiOS` (kernel contract), never on each other's internals.** The dependency graph is audited on every PR.

2. **All code must pass `clippy -D warnings`.** No exceptions, no `#[allow]` for clippy lints without justification in comments.

3. **All public APIs must have doc comments.** Internal functions should be self-documenting through clear naming.

4. **Event sourcing through Lago is the sole persistence path.** No ad-hoc file writes for agent state.

5. **Port traits define boundaries.** `EventStorePort`, `ModelProviderPort`, `ToolHarnessPort`, `PolicyGatePort`, `ApprovalPort` — subsystems implement ports, never import concrete types from other subsystems.

## Code Style

- **Rust 2024 Edition** idioms
- `snake_case` for functions and variables, `PascalCase` for types
- Prefer `thiserror` for error types, `anyhow` only in binary crates
- Use `tracing` for instrumentation (not `log` or `println!`)
- Keep functions short — if it needs a comment block explaining what it does, it probably needs to be split

## Commit Messages

```
subsystem: short description

Longer explanation if needed. Reference issues with #123.
```

Examples:
- `arcan: add streaming response support for Anthropic provider`
- `lago: fix journal compaction race condition`
- `aiOS: add ApprovalPort to kernel contract`

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).

## Questions?

Open a [Discussion](https://github.com/broomva/life/discussions) or reach out at [broomva.tech](https://broomva.tech).
