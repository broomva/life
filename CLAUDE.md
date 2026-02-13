# Lago - Unified Agent Persistence Layer

## Project Overview
Lago is the canonical storage and transport backbone for long-lived AI agents.
All agent state changes (tool use, file writes, messages, memory) are consolidated
into a single event-sourced, versioned system with streaming I/O.

## Technology
- **Language**: Rust (edition 2024)
- **Storage**: redb v2 (embedded, ACID, pure Rust)
- **gRPC**: tonic + prost
- **HTTP/SSE**: axum + tower
- **IDs**: ULID (time-sortable)
- **Compression**: zstd for blob storage
- **Hashing**: SHA-256 for content addressing

## Build Commands
```bash
cargo build --workspace          # Build everything
cargo test --workspace           # Run all tests
cargo clippy --workspace         # Lint
cargo fmt --check                # Format check
```

## Workspace Layout
- `crates/lago-core` - Foundation types, traits, errors (zero deps)
- `crates/lago-journal` - Event journal (redb-backed)
- `crates/lago-store` - Content-addressed blob storage
- `crates/lago-fs` - Filesystem manifest + branching
- `crates/lago-ingest` - gRPC streaming ingest
- `crates/lago-api` - HTTP REST + SSE streaming
- `crates/lago-policy` - Policy engine + security
- `crates/lago-cli` - CLI tool
- `crates/lagod` - Daemon binary

## Key Patterns
- Event sourcing: all state derived from append-only event journal
- Content-addressed blobs: SHA-256 hash as key, zstd compressed
- SSE format trait: OpenAI, Anthropic, Vercel AI SDK compatibility
- Policy engine: rule-based tool governance with RBAC
