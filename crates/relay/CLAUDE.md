# Life Relay

**Version**: 0.1.0 | **Rust**: edition 2024, MSRV 1.85
**Tests**: 3 passing | 3 crates

Web-based remote access to agent sessions (Claude Code, Codex, Arcan) via broomva.tech.
Rust relay daemon (`relayd`) connects outbound to broomva.tech via WebSocket, bridging
local agent sessions to the web UI.

## Build & Verify
```bash
cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace
```

## Stack
Rust 2024 | axum (HTTP) | tokio-tungstenite (WS) | portable-pty (PTY) | serde

## Crates
- **life-relay-core** — Types, wire protocol, errors (shared between daemon and server)
- **life-relay-api** — Local HTTP API server (health, session list)
- **life-relayd** — Daemon binary (CLI, WS client, session adapters)

## Running
```bash
cargo run -p life-relayd -- start --bind 127.0.0.1:3004
cargo run -p life-relayd -- auth --url https://broomva.tech
cargo run -p life-relayd -- status
```

## Rules
- Formatting: `cargo fmt` before every commit
- Linting: `cargo clippy --workspace -- -D warnings`
- Testing: All new code requires tests; `cargo test --workspace` must pass
- Safe Rust: No `unsafe` unless absolutely necessary
- Error handling: `thiserror` for libraries, `anyhow` for binaries
- Naming: snake_case (functions), PascalCase (types), SCREAMING_SNAKE_CASE (constants)
- Rust 2024 Edition: `gen` is reserved; `set_var`/`remove_var` are `unsafe`
