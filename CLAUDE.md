# Praxis — Canonical Tool Execution Engine

**Version**: 0.1.0 | **Date**: 2026-03-19 | **Status**: Active (Phase 4 — MCP server complete)
**Tests**: 90 passing | 4 crates | Rust 2024 Edition (MSRV 1.85)

Praxis is the canonical tool execution and sandbox engine for the Agent OS. It implements the `Tool` trait from `aios-protocol` and provides filesystem, editing, shell, memory, MCP server/client, and skill discovery tools.

## Architecture

```
aios-protocol (Tool trait, ToolDefinition, ToolCall, ToolResult, ToolError)
  │
  └── praxis-core (sandbox policy, workspace enforcement, command runner)
        │
        ├── praxis-tools (ReadFile, WriteFile, ListDir, Glob, Grep, EditFile, Bash, ReadMemory, WriteMemory)
        ├── praxis-skills (SKILL.md parser, SkillRegistry)
        └── praxis-mcp (MCP server + client via rmcp 0.15)
              ├── server: PraxisMcpServer (ServerHandler — exposes ToolRegistry as MCP)
              ├── transport: stdio + Streamable HTTP (axum) transport helpers
              ├── convert: bidirectional canonical ↔ MCP type conversions
              ├── connection: MCP client (connect to external MCP servers)
              └── tool: McpTool bridge (external MCP tools as canonical Tools)
```

## Crates

### praxis-core (12 tests)
- **SandboxPolicy**: cwd validation, env filtering, timeout enforcement, output truncation
- **FsPolicy**: workspace boundary enforcement via canonicalize + starts_with
- **CommandRunner**: trait + LocalCommandRunner implementation
- **Error types**: PraxisError (thiserror)

### praxis-tools (24 tests)
- **ReadFileTool**: reads files with hashline tags for content-addressed editing
- **WriteFileTool**: writes files within workspace boundary
- **ListDirTool**: lists directory contents with metadata
- **GlobTool**: pattern-based file search using `glob` crate
- **GrepTool**: regex search with glob filtering and match limits
- **EditFileTool**: hashline (Blake3) content-addressed line editing
- **BashTool**: shell command execution within sandbox constraints
- **ReadMemoryTool / WriteMemoryTool**: agent memory persistence (file-based markdown)

### praxis-skills (11 tests)
- **SkillMetadata**: parsed from SKILL.md YAML frontmatter
- **parse_skill_md**: frontmatter extractor + validator
- **SkillRegistry**: directory discovery, activation, system prompt catalog generation

### praxis-mcp (34 tests — 24 unit + 9 integration + 1 doctest)

**Server** (exposing tools to MCP clients):
- **PraxisMcpServer**: implements rmcp `ServerHandler`, wraps `ToolRegistry` as an MCP server
- **serve_stdio**: run MCP server over stdin/stdout (for CLI integration, e.g. Claude Desktop)
- **serve_http / mcp_axum_router**: run MCP server over Streamable HTTP (axum)
- **HttpTransportConfig**: bind address, endpoint path, stateful sessions, cancellation

**Client** (consuming external MCP servers):
- **McpServerConfig**: server configuration with env/args
- **connect_mcp_stdio**: spawns MCP server subprocess via rmcp transport
- **McpTool**: bridges external MCP server tools to canonical Tool trait
- **mcp_tool_to_definition**: converts rmcp Tool to canonical ToolDefinition

**Conversions**:
- **definition_to_mcp_tool**: canonical ToolDefinition → rmcp Tool
- **tool_result_to_call_result**: canonical ToolResult → rmcp CallToolResult

## Dependency Rules

Praxis depends **only** on `aios-protocol` from aiOS. It must NOT depend on:
- Arcan crates (`arcan-*`, `arcand`)
- Lago crates (`lago-*`, `lagod`)
- Autonomic crates (`autonomic-*`, `autonomicd`)

These rules are enforced by `scripts/architecture/verify_dependencies.sh`.

## Commands

```bash
cargo fmt                                      # Format
cargo clippy --workspace -- -D warnings        # Lint
cargo test --workspace                         # Run all tests
cargo test -p praxis-core                      # Test specific crate
cargo test -p praxis-tools                     # Test tools
cargo test -p praxis-skills                    # Test skills
cargo test -p praxis-mcp                       # Test MCP bridge
```

## Key Patterns

- **Hashline editing**: Files are read with Blake3 content hashes per line. Edit operations must reference the correct hash tag, preventing "blind" edits on stale content.
- **Workspace boundary enforcement**: All filesystem operations canonicalize paths and verify they start with the workspace root. Symlink traversal attacks are blocked.
- **Sandbox policy**: Shell commands are constrained by timeout, output size limits, env filtering, and cwd validation. Shell execution can be disabled entirely.
- **Memory key validation**: Memory keys must be alphanumeric/hyphens/underscores only. No path traversal (`..`), no hidden files (`.`prefix).

## Key Patterns (MCP Server)

- **Transport-agnostic server**: `PraxisMcpServer` is decoupled from transport. Use `serve_stdio()` for CLI, `serve_http()` for network, or `mcp_axum_router()` to compose with other axum routes.
- **In-process testing**: Use `tokio::io::duplex` to create client↔server pairs for integration tests without spawning subprocesses.
- **Factory pattern for HTTP**: The Streamable HTTP transport accepts a factory closure `Fn() -> Result<PraxisMcpServer>` to create a fresh server per session.

## Integration Plan

- **Phase 4** (complete): MCP server implementation — expose any `ToolRegistry` as an MCP server over stdio and HTTP.
- **Phase 5** (pending): Wire Arcan to use Praxis as the canonical tool backend, replacing direct arcan-harness tool implementations.
- **Phase 6** (pending): Unify sandbox abstractions across aiOS and Lago with Praxis as the single source of truth.
