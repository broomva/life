# Praxis

Canonical tool execution and sandbox engine for the [Agent OS](https://github.com/broomva).

Praxis implements the `Tool` trait from `aios-protocol` and provides a secure, sandboxed environment for executing filesystem operations, shell commands, memory persistence, MCP bridge connections, and skill discovery within agent sessions.

## Architecture

```
aios-protocol (Tool trait, ToolDefinition, ToolCall, ToolResult, ToolError)
  |
  +-- praxis-core        Sandbox policy, workspace enforcement, command runner
  |
  +-- praxis-tools       ReadFile, WriteFile, ListDir, Glob, Grep, EditFile, Bash, Memory
  |
  +-- praxis-skills      SKILL.md parser, SkillRegistry, system prompt catalog
  |
  +-- praxis-mcp         MCP server connection + McpTool bridge (via rmcp)
```

## Key Features

- **Hashline editing** -- content-addressed (Blake3) line editing prevents blind edits on stale content.
- **Workspace boundary enforcement** -- all filesystem operations canonicalize paths and block symlink traversal attacks.
- **Sandbox policy** -- shell commands are constrained by timeout, output size limits, env filtering, and cwd validation.
- **Skill discovery** -- parses `SKILL.md` frontmatter to build a skill registry and system prompt catalog.
- **MCP bridge** -- connects to MCP servers over stdio and bridges their tools to the canonical `Tool` trait.

## Getting Started

```bash
cargo test --workspace          # Run all 49 tests
cargo clippy --workspace -- -D warnings   # Lint
cargo fmt                       # Format
```

## Crates

| Crate | Tests | Purpose |
|-------|-------|---------|
| `praxis-core` | 12 | Sandbox policy, filesystem policy, command runner |
| `praxis-tools` | 24 | Canonical tool implementations |
| `praxis-skills` | 11 | SKILL.md parsing and skill registry |
| `praxis-mcp` | 2 | MCP server bridge via rmcp |

## Requirements

- Rust 2024 edition (MSRV 1.85)
- Depends only on `aios-protocol` from the Agent OS stack

## License

[MIT](LICENSE)
