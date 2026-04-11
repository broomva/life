# Life CLI

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![docs](https://img.shields.io/badge/docs-broomva.tech-purple.svg)](https://docs.broomva.tech/docs/life)

**Deployment and onboarding CLI for the Life Agent OS** -- configure providers, deploy agents to cloud, manage relay sessions.

The `life` binary is the operator's entry point. It handles interactive onboarding (`life setup`), cloud deployment (`life deploy`), monitoring (`life status`, `life logs`), and relay management.

## Crate

| Crate | Purpose |
|-------|---------|
| `life-cli` | CLI logic: setup wizard, deploy, status, logs, scale, cost, relay |

## Quick Start

```bash
# Install
cargo install life-os

# Interactive setup (provider selection, API key, connection test)
life setup

# Deploy an agent
life deploy

# Check status
life status
```

## Commands

| Command | What It Does |
|---------|-------------|
| `life setup` | Interactive onboarding wizard (provider, API key, model) |
| `life deploy` | Deploy an agent configuration to cloud |
| `life status` | Show deployed agent status |
| `life logs` | Stream service logs |
| `life scale` | Scale agent services |
| `life cost` | Cost tracking |
| `life relay` | Manage relay daemon |

## Build and Test

```bash
cargo fmt && cargo clippy -p life-cli -- -D warnings && cargo test -p life-cli
```

## Documentation

- [CLAUDE.md](../../CLAUDE.md) -- workspace context

## License

[MIT](../../LICENSE)
