# Life Relay — Web-Based Remote Agent Sessions

> Open-source remote access to agent sessions (Claude Code, Codex, Arcan) via broomva.tech PWA.
> Inspired by [Omnara](https://omnara.com) but multi-provider, self-hostable, and integrated into the Life Agent OS.

## Problem

Agent CLI sessions (Claude Code, Codex, Arcan) run locally in a terminal. Monitoring, approving permissions, or sending input requires physical access to the machine. Omnara solves this with a $20/mo proprietary web/mobile app. We want the same capability as part of the open-source Life ecosystem.

## Architecture

```
Browser (broomva.tech PWA)
    |
    +-- SSE <-- /api/relay/sessions/{id}/stream   (output)
    +-- POST -> /api/relay/sessions/{id}/input    (commands)
         |
    +----+----------------------------+
    |   Redis Pub/Sub + Postgres      |  (Vercel infra)
    +----+----------------------------+
         | WebSocket (outbound, TLS+JWT)
    +----+----------------------------+
    |   relayd  (Rust daemon)         |  (user's machine)
    |                                 |
    |   Adapters:                     |
    |   +-- ArcanAdapter  -> arcand HTTP API (SSE events)
    |   +-- ClaudeAdapter -> tmux/PTY (output capture + keystrokes)
    |   +-- CodexAdapter  -> tmux/PTY
    +----------------------------------+
```

### Key Design Decisions

1. **Outbound WebSocket relay** — relayd connects *out* to broomva.tech. Works behind NAT/firewalls with zero user configuration.
2. **Redis pub/sub bridges WS<->SSE** — matches broomva.tech's existing `resumable-stream` pattern from chat.
3. **Separate from arcand** — relay is a connectivity layer, not an agent runtime. It *calls* arcand's HTTP API for Arcan sessions.
4. **Device Authorization for registration** — reuses existing `/device` page and `deviceAuthCode` table.
5. **Wire protocol** — JSON over WebSocket, bidirectional `ServerMessage`/`DaemonMessage` enums.

## Crate: `life-relay`

Location: `core/life/relay/`

```
relay/
  Cargo.toml          # tokio, tokio-tungstenite, portable-pty, serde, reqwest
  src/
    main.rs           # CLI: relayd auth | start | stop | status
    config.rs         # ~/.config/life/relay/ config + credentials
    auth.rs           # Device auth flow (opens browser, polls for token)
    connection.rs     # WS client with auto-reconnect + exponential backoff
    protocol.rs       # Wire protocol types (serde JSON)
    registry.rs       # Local session registry (JSON file)
    daemon.rs         # Main event loop: WS recv -> dispatch to adapters
    adapters/
      mod.rs          # SessionAdapter trait
      arcan.rs        # HTTP client to local arcand
      claude.rs       # Claude Code via tmux/PTY
      codex.rs        # Codex CLI via tmux/PTY
      pty.rs          # Shared portable-pty helpers
```

### Dependencies

- `tokio` + `tokio-tungstenite` — async runtime + WebSocket
- `portable-pty` — cross-platform PTY spawning (same as Mission Control)
- `reqwest` + `reqwest-eventsource` — HTTP + SSE client for ArcanAdapter
- `serde` + `serde_json` — wire protocol serialization
- `clap` — CLI argument parsing

## Wire Protocol

```rust
// Server -> Daemon (commands from web UI)
enum ServerMessage {
    Spawn { session_type: String, config: SpawnConfig },
    Input { session_id: String, data: String },
    Resize { session_id: String, cols: u16, rows: u16 },
    Approve { session_id: String, approval_id: String, approved: bool },
    Kill { session_id: String },
    ListSessions,
    Ping,
}

// Daemon -> Server (events from local sessions)
enum DaemonMessage {
    Output { session_id: String, data: String, seq: u64 },
    SessionCreated { session: SessionInfo },
    SessionEnded { session_id: String, reason: String },
    ApprovalRequest { session_id: String, approval_id: String, capability: String },
    SessionList { sessions: Vec<SessionInfo> },
    NodeInfo { name: String, hostname: String, capabilities: Vec<String> },
    Pong,
    Error { code: String, message: String },
}
```

## Auth Flow

1. User logs into broomva.tech (Better Auth)
2. Installs relayd: `cargo install life-relay`
3. `relayd auth` -> opens browser to `/device?code=XXXX`
4. User approves -> JWT stored in `~/.config/life/relay/credentials.json`
5. `relayd start` -> outbound WS to `wss://broomva.tech/api/relay/connect`
6. Console shows node online at `/console/relay`

## Session Types

| Type | Adapter | How it works |
|------|---------|--------------|
| `arcan` | ArcanAdapter | HTTP proxy to local `arcand` — SSE events forwarded as WS frames |
| `claude-code` | ClaudeAdapter | PTY spawn of `claude` CLI — continuous output capture + keystroke injection |
| `codex` | CodexAdapter | PTY spawn of `codex` CLI — same pattern as Claude |

## broomva.tech Integration

### API Routes

| Route | Method | Purpose |
|-------|--------|---------|
| `/api/relay/connect` | WS | relayd connects here |
| `/api/relay/nodes` | GET | List user's relay nodes |
| `/api/relay/sessions` | GET | List sessions across nodes |
| `/api/relay/sessions/[id]/stream` | GET | SSE stream of session output |
| `/api/relay/sessions/[id]/input` | POST | Send input to session |
| `/api/relay/sessions/[id]/spawn` | POST | Spawn new session |

### Console Pages

- `/console/relay` — node list + session overview
- `/console/relay/[nodeId]` — node detail with sessions
- `/console/relay/session/[id]` — terminal view + input bar

### Database Tables

- `RelayNode` — registered machines (userId, name, hostname, status, lastSeenAt)
- `RelaySession` — sessions (nodeId, sessionType, status, workdir, remoteSessionId, lastSequence, model)

## Comparison with Omnara

| | Omnara | Life Relay |
|---|--------|------------|
| Price | $20/mo | Free (open source) |
| Providers | Claude Code, Codex | Claude Code, Codex, Arcan |
| Access | Proprietary app | broomva.tech PWA |
| Voice | Built-in | Not in MVP |
| Self-host | No | Yes (Docker compose) |
| Auth profiles | Single account | Per-session profiles |

## Comparison with claude-remote-sessions

The `claude-remote-sessions` skill (Discord/Telegram) is the **precursor**. Relay evolves the concept:

| | claude-remote-sessions | Life Relay |
|---|----------------------|------------|
| UI | Discord/Telegram | Web browser (PWA) |
| Transport | Discord plugin MCP | WebSocket relay |
| Session management | tmux + bash scripts | Rust daemon (portable-pty) |
| Multi-provider | Claude Code only | Claude Code + Codex + Arcan |
| Auth | Per-channel OAuth profiles | Device auth + JWT |
| Streaming | tmux pane capture (polling) | Continuous PTY read (real-time) |

## Implementation Phases

- **Phase 1 (MVP)**: relayd scaffold + ClaudeAdapter + WS relay + basic console UI
- **Phase 2**: ArcanAdapter, CodexAdapter, xterm.js, approvals, reconnection
- **Phase 3**: Multi-node, PWA push, SpacetimeDB discovery, self-hosting, Lago recording

## Linear Project

Tracked in Linear: [Life Relay — Remote Agent Sessions](https://linear.app/broomva/project/life-relay-remote-agent-sessions-e40523e79ad0)

Tickets: BRO-264 through BRO-280 (16 issues across 3 phases)
