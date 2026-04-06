# Unified `.life/` Filesystem — Design Spec

**Date**: 2026-04-06
**Status**: Approved
**Replaces**: `.arcan/`, `.lago/`, `.control/` as separate directories

## Problem

Each daemon uses its own directory (`.arcan/`, `.lago/`, `.control/`), its own config format, and its own credential storage. API keys are stored in plaintext in `~/.life/config.toml`. There's no shared path resolution, no `life init`, and no unified knowledge graph wiring. A new user must understand 5+ different directory conventions.

## Solution

Consolidate all Life framework state into a two-tier filesystem:
- **`~/.life/`** — global user config, credentials, skills (like `~/.gitconfig`)
- **`.life/`** — per-project state for all daemons (like `.git/`)

Hard cut — no legacy `.arcan/`, `.lago/` support.

## Architecture

### Global Home: `~/.life/`

```
~/.life/
├── config.toml                       # Non-secret settings
├── credentials/
│   ├── keychain.toml                 # Keychain service references
│   └── .env                          # Fallback: plaintext env vars (0600)
├── skills/                           # Global skill definitions
│   └── *.md
├── agents/                           # Agent personas/identities
│   └── default.toml
└── logs/
    └── life.log
```

**`config.toml`** — no secrets, safe to back up:
```toml
[provider]
name = "anthropic"
model = "claude-sonnet-4-5-20250929"

[consciousness]
enabled = true

[arcan]
port = 3000

[lago]
grpc_port = 50051
http_port = 8080

[autonomic]
port = 3002

[haima]
port = 3003
```

### Per-Project: `.life/`

Created by `life init`. Discovered by walking up from cwd (same as git).

```
.life/
├── config.toml                       # Project-level overrides (committable)
├── .env                              # Project-level secrets (gitignored)
│
├── arcan/                            # Agent runtime state
│   ├── journal.redb
│   ├── blobs/
│   ├── memory/
│   ├── sessions/
│   └── last_session
│
├── lago/                             # Persistence substrate
│   ├── journal.redb
│   ├── blobs/
│   └── snapshots/
│
├── autonomic/                        # Homeostasis state
│   └── state.json
│
├── haima/                            # Finance state
│   └── wallet.enc
│
├── knowledge/                        # Knowledge graph
│   ├── index.lance/
│   └── graph.json
│
├── control/                          # Governance metalayer (committable)
│   ├── policy.yaml
│   ├── topology.yaml
│   ├── commands.yaml
│   └── state.json
│
├── skills/                           # Project-local skills
│   └── *.md
│
└── logs/
    ├── arcan.log
    ├── lago.log
    └── autonomic.log
```

### Gitignore Contract

`.life/` added to `.gitignore` by `life init`, with exceptions:
```gitignore
# Life Agent OS
.life/
!.life/config.toml
!.life/control/
```

This means:
- `.life/config.toml` — committable (no secrets)
- `.life/control/` — committable (governance is shared)
- Everything else — gitignored (runtime data, journals, secrets)

## Credential Resolution

Cascading, in order:

1. **Project `.life/.env`** — per-project overrides
2. **System keychain** — `life/anthropic_api_key` service entry (macOS Keychain, Linux secret-service)
3. **`~/.life/credentials/.env`** — user-level fallback
4. **Environment variables** — `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.

`life setup` writes to both the keychain (if available) and `~/.life/credentials/.env` (fallback). The `.env` file has `0600` permissions.

**Keychain integration:**
- macOS: `security add-generic-password -s life -a anthropic_api_key -w <key>`
- Linux: `secret-tool store --label='life/anthropic' service life key anthropic_api_key`
- Fallback: `~/.life/credentials/.env` with `ANTHROPIC_API_KEY=sk-ant-...`

`keychain.toml` records which keys are stored where:
```toml
[anthropic]
storage = "keychain"  # or "env_file" or "env_var"
service = "life"
account = "anthropic_api_key"

[openai]
storage = "env_file"
```

## Path Resolution

### Shared `life-paths` crate

New crate: `crates/life-paths/` (zero external deps beyond `dirs`).

```rust
/// Find the .life/ directory by walking up from cwd.
/// Returns None if not found (use ~/.life/ as fallback).
pub fn find_project_root() -> Option<PathBuf>;

/// Resolve the data directory for a module.
/// Priority: CLI flag > project .life/{module}/ > ~/.life/{module}/
pub fn resolve_module_dir(module: &str, cli_override: Option<&Path>) -> PathBuf;

/// Resolve a credential by cascading through sources.
pub fn resolve_credential(key: &str) -> Option<String>;

/// Load .env file and merge into environment.
pub fn load_env(path: &Path);
```

### Discovery Algorithm

```
fn find_project_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir();
    loop {
        if dir.join(".life").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;  // reached filesystem root
        }
    }
}
```

### Daemon Changes

Each daemon's `--data-dir` default changes from `.{module}/` to `.life/{module}/`:

| Daemon | Old default | New default |
|--------|-------------|-------------|
| arcan | `.arcan/` | `.life/arcan/` |
| lagod | `.lago/` | `.life/lago/` |
| autonomicd | `{data_dir}/` | `.life/autonomic/` |
| haimad | `{data_dir}/` | `.life/haima/` |

All use `life_paths::resolve_module_dir()`. CLI flag override still works for backward compat.

## `life init`

Creates `.life/` in current directory:

```bash
$ life init
  ✓ Created .life/
  ✓ Created .life/config.toml (from ~/.life/config.toml defaults)
  ✓ Created .life/control/policy.yaml (default governance)
  ✓ Updated .gitignore
```

Creates minimal structure:
```
.life/
├── config.toml
└── control/
    └── policy.yaml
```

Subdirectories (`arcan/`, `lago/`, etc.) are created on first daemon run.

## `life setup` Changes

Updated flow:
1. Show banner + system info
2. Select provider
3. Prompt for API key
4. Store in keychain (try first) or `~/.life/credentials/.env` (fallback)
5. Record storage method in `~/.life/credentials/keychain.toml`
6. Write `~/.life/config.toml` (no secrets)
7. Test connection
8. Show success

## Knowledge Graph Wiring

`.life/knowledge/` is the project's semantic index:
- `index.lance/` — vector embeddings fed by Lago events
- `graph.json` — wikilink graph from `docs/**/*.md`

Fed by:
- `EventKind::MemoryCommitted` → updates `index.lance/`
- `EventKind::ObservationAppended` → updates `graph.json`
- `lago-knowledge` crate does the indexing

Queryable by:
- `arcan`'s `memory_query` tool reads `.life/knowledge/`
- `life search` CLI command (future)

Uses the same frontmatter schema as `knowledge-graph-memory` skill:
```yaml
---
name: "memory name"
description: "one-line description"
type: user | feedback | project | reference
---
```

## Control Metalayer

`.life/control/` replaces repo-root `.control/`:
- Same YAML schema (policy.yaml, topology.yaml, commands.yaml)
- Committable — governance is shared across the team
- `life init` copies default from bstack skill template
- `control-metalayer-loop` skill reads from `.life/control/`
- `make control-audit` updated to check `.life/control/`

## Implementation Phases

1. **`life-paths` crate** — shared path resolution, credential cascade, .env loading
2. **`life init` command** — create `.life/`, scaffold control, update .gitignore
3. **Update `life setup`** — keychain storage, no secrets in config.toml
4. **Update arcan** — use `life_paths::resolve_module_dir("arcan")`
5. **Update lago, autonomic, haima** — same pattern
6. **Update `.life/control/`** — move from repo root, update references
7. **Wire knowledge graph** — `.life/knowledge/` fed by Lago events

## Key Files to Modify

- `crates/life-paths/` — NEW crate
- `crates/cli/life-cli/src/setup.rs` — credential storage changes
- `crates/cli/life-cli/src/cli.rs` — add `Init` command
- `crates/arcan/arcan/src/main.rs` — data dir resolution
- `crates/lago/lagod/src/config.rs` — data dir resolution
- `crates/autonomic/autonomicd/src/main.rs` — data dir resolution
- `crates/haima/haimad/src/main.rs` — data dir resolution
- `CLAUDE.md` — update governance stack paths
- `.gitignore` — update patterns
