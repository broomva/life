# Relay

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)
[![docs](https://img.shields.io/badge/docs-broomva.tech-purple.svg)](https://docs.broomva.tech/docs/life/relay)

**Remote agent session access for the Life Agent OS** -- connects local agent sessions (Arcan, Claude Code, Codex) to a web UI via WebSocket relay.

Relay enables browser-based access to agent sessions running on any machine. The daemon (`relayd`) connects outbound to broomva.tech, bridging local PTY sessions to the web interface.

## Crates

| Crate | Purpose |
|-------|---------|
| `life-relay-core` | Wire protocol types, errors (shared between daemon and server) |
| `life-relay-api` | Local HTTP API server (health, session listing) |
| `life-relayd` | Daemon binary: WS client, session adapters, CLI |

## Quick Start

```bash
# Start the relay daemon
cargo run -p life-relayd -- start --bind 127.0.0.1:3004

# Authenticate with broomva.tech
cargo run -p life-relayd -- auth --url https://broomva.tech

# Check daemon status
cargo run -p life-relayd -- status
```

## Build and Test

```bash
cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace
```

## Documentation

- [CLAUDE.md](CLAUDE.md) -- full technical context
- [docs/RELAY.md](../../docs/RELAY.md) -- architecture and design

## License

[MIT](../../LICENSE)
