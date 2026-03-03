# Architecture

## Purpose

Spaces is a Discord-like real-time communication platform built entirely on SpacetimeDB 2.0. The server-side logic runs as a WASM module inside SpacetimeDB, and clients connect via WebSocket using the SpacetimeDB SDK. The system supports servers, channels, threads, messages, reactions, role-based authorization, and agent integration.

## Boundaries

| Boundary | Input | Output | Owner |
|---|---|---|---|
| SpacetimeDB Module (WASM) | Reducer calls via WebSocket | Table row inserts/updates/deletes | `spacetimedb/src/` |
| Authorization Layer | ReducerContext (ctx.sender, server_id) | Role check result or error string | `spacetimedb/src/auth.rs` |
| Type System | Enum variants (MemberRole, ChannelType, MessageType) | Validated domain types | `spacetimedb/src/types.rs` |
| Table Schema | Reducer mutations | Persistent rows with btree indexes | `spacetimedb/src/tables.rs` |
| Reducer Layer | Validated inputs + authorization | Table mutations + log events | `spacetimedb/src/reducers/` |
| Client SDK Bindings | Generated from WASM module | Rust types + reducer call stubs | `src/module_bindings/` (generated) |
| CLI Client | User keyboard input + subscription callbacks | Terminal output + reducer calls | `src/main.rs` |

## Data Shape Contracts

- SpacetimeDB enforces schema at the table level: column types, primary keys, unique constraints, and btree indexes are declared in Rust structs and compiled into the WASM module.
- Custom types (`MemberRole`, `ChannelType`, `MessageType`) use `#[derive(SpacetimeType)]` and are serialized/deserialized automatically across the WebSocket boundary.
- Client bindings are auto-generated from the module schema via `spacetime generate` — never hand-edited.
- All table access on the server uses typed accessors: `ctx.db.table_name().column().find()` / `.filter()` / `.insert()` / `.update()`.

## Module Ownership Rules

| Module | Owner | Responsibility |
|---|---|---|
| `spacetimedb/src/types.rs` | Schema | Domain enums shared across all reducers |
| `spacetimedb/src/tables.rs` | Schema | All 11 table definitions |
| `spacetimedb/src/auth.rs` | Auth | Role hierarchy, membership checks, channel-to-server resolution |
| `spacetimedb/src/reducers/users.rs` | Users | Profile management, agent registration |
| `spacetimedb/src/reducers/servers.rs` | Servers | Server CRUD, membership, role management |
| `spacetimedb/src/reducers/channels.rs` | Channels | Channel CRUD with cascade deletes |
| `spacetimedb/src/reducers/threads.rs` | Threads | Thread creation and archival |
| `spacetimedb/src/reducers/messages.rs` | Messages | Send/edit/delete messages, reactions, typing, read state |
| `spacetimedb/src/lib.rs` | Lifecycle | init (seed data), client_connected, client_disconnected |
| `src/main.rs` | Client | CLI interaction, subscription management, state display |

## Execution Flow

1. Entry: Client connects via WebSocket to `https://maincloud.spacetimedb.com`, database `spaces`
2. Lifecycle: SpacetimeDB calls `client_connected` reducer — upserts UserPresence, creates UserProfile if new, auto-joins Spaces Hub
3. Subscription: Client subscribes to all public tables via `subscribe_to_all_tables()` — receives initial snapshot, then real-time diffs
4. User action: CLI parses command (e.g. `/channel general`) or message text, calls appropriate reducer
5. Reducer execution: SpacetimeDB runs reducer in WASM sandbox — validates via auth.rs, mutates tables, logs
6. Propagation: Table changes propagate to all subscribed clients via WebSocket
7. Display: Client's `on_insert` / `on_update` / `on_delete` callbacks render changes to terminal

## Refactor Checklist

- [ ] Boundary contracts unchanged or versioned (table schema changes require `--clear-database` or migration)
- [ ] Ownership map still accurate (new reducers documented in appropriate module)
- [ ] Bindings regenerated after any schema change (`spacetime generate`)
- [ ] `make smoke` passes (WASM compile + client compile)
- [ ] Documentation updated in same change
