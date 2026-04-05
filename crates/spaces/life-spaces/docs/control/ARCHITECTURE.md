# Control-Aware Architecture

## Boundaries

- **WebSocket boundary**: Client connects to SpacetimeDB maincloud. All reducer calls are serialized, authenticated by SpacetimeDB (ctx.sender() is the verified identity). Client receives table diffs via subscription.
- **WASM sandbox boundary**: Module runs inside SpacetimeDB's WASM runtime. No filesystem, network, timers, or non-deterministic operations. All state lives in tables. All mutations happen through reducers.
- **Authorization boundary**: `auth.rs` enforces role hierarchy before any mutation. Every reducer that modifies server/channel/thread/message data checks membership and role level via `require_member()` or `require_role()`.
- **Schema boundary**: Table definitions in `tables.rs` are the source of truth. Client bindings are generated from the compiled WASM module. Any schema change requires binding regeneration and client rebuild.
- **Persistence boundary**: SpacetimeDB handles all persistence. Tables are the database. No external databases, caches, or queues.

## Ownership

### Product Modules (own product behavior)

| Module | Responsibility |
|---|---|
| `spacetimedb/src/types.rs` | Domain enums (MemberRole, ChannelType, MessageType) |
| `spacetimedb/src/tables.rs` | Schema: 11 tables with indexes and constraints |
| `spacetimedb/src/reducers/` | All business logic: users, servers, channels, threads, messages |
| `spacetimedb/src/auth.rs` | Authorization: role checks, membership validation |
| `spacetimedb/src/lib.rs` | Lifecycle: init seed data, connect/disconnect handlers |
| `src/main.rs` | CLI client: user interaction, display, reducer calls |

### Control Modules (own governance and reliability behavior)

| Module | Responsibility |
|---|---|
| `scripts/harness/` | Build verification: smoke, test, lint, typecheck |
| `scripts/control/` | Control gates: smoke, check, test with auto-detection |
| `scripts/audit_*.sh` | Audit scripts: verify harness and control artifacts exist and are complete |
| `.github/workflows/` | CI automation: run `make ci` on push |
| `Makefile` | Command surface: unified `make` targets for all operations |
| `docs/` | Architecture and observability documentation |
| `PLANS.md` / `METALAYER.md` | Development planning and control loop configuration |
