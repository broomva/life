# PLANS.md

Use this file for multi-step work where durable context matters.

## Objective

- Outcome: A production-grade Discord clone ("Spaces") running on SpacetimeDB 2.0 as the communication layer for the aiOS/Arcan/Lago agentic system network.
- Why it matters: Provides real-time human-to-human, human-to-agent, and agent-to-agent communication with servers, channels, threads, role-based permissions, and full message lifecycle.
- Non-goals: Voice/video streaming, file uploads, rich embeds, OAuth/SSO integration (future work).

## Constraints

- Runtime/tooling constraints: SpacetimeDB 2.0.2 WASM module (deterministic reducers, no network/filesystem), Rust CLI client using spacetimedb-sdk (blocking I/O), wasm32-unknown-unknown compilation target.
- Security/compliance constraints: All authorization enforced server-side via `ctx.sender()`, role hierarchy (Owner > Admin > Moderator > Member/Agent), no client-trusted identity.
- Performance/reliability constraints: All tables public for real-time subscriptions, btree indexes on foreign-key columns (server_id, channel_id, message_id), event tables (TypingIndicator, SystemNotification) for transient data.

## Context Snapshot

- Relevant files/modules:
  - `spacetimedb/src/lib.rs` — module root, init + lifecycle reducers
  - `spacetimedb/src/types.rs` — MemberRole, ChannelType, MessageType enums
  - `spacetimedb/src/tables.rs` — 11 table definitions (UserProfile, UserPresence, Server, ServerMember, Channel, Thread, Message, Reaction, ChannelReadState, TypingIndicator, SystemNotification)
  - `spacetimedb/src/auth.rs` — authorization helpers (get_membership, require_role, get_server_for_channel)
  - `spacetimedb/src/reducers/` — users, servers, channels, threads, messages
  - `src/main.rs` — Rust CLI client with server/channel navigation
  - `src/module_bindings/` — auto-generated client bindings (44 files)
- Existing commands/workflows: `make smoke`, `make test`, `make lint`, `make ci`, `spacetime publish spaces`
- Known risks: Subscription race condition on piped stdin (mitigated with AtomicBool ready gate), credentials file corruption on first run (mitigated with graceful fallback).

## Execution Plan

1. Step: Implement SpacetimeDB module with full schema and reducers
   - Expected output: Module compiles to wasm32-unknown-unknown, publishes to maincloud
   - Verification: `spacetime publish spaces --clear-database -y`, `spacetime logs spaces` shows init reducer ran
2. Step: Generate client bindings and implement CLI client
   - Expected output: 44 binding files generated, CLI connects and displays servers/channels/messages
   - Verification: `cargo run` connects, `/servers` lists Spaces Hub, messages send and appear cross-client
3. Step: Apply harness and control metalayer
   - Expected output: `make smoke` passes, both harness and control audits pass
   - Verification: `scripts/audit_harness.sh .`, `scripts/audit_control.sh .`

## Checkpoints

- [x] Baseline captured
- [x] Implementation complete (module + CLI)
- [x] Static checks passed (smoke, harness audit, control audit)
- [ ] Tests passed (unit tests not yet written)
- [x] Docs updated

## Decision Log

- 2026-03-02:
  - Decision: Use column-level `#[index(btree)]` instead of table-level index declarations
  - Reason: Table-level `index(name=..., btree(...))` requires complex `accessor` fields; column-level auto-generates accessors named after the column
  - Alternatives considered: Table-level index macro with string literal names and explicit accessors

- 2026-03-02:
  - Decision: Auto-join Spaces Hub on first client_connected
  - Reason: New users should immediately see content without manual /join
  - Alternatives considered: Require explicit /join, show empty state

- 2026-03-02:
  - Decision: AtomicBool ready gate for subscription race condition
  - Reason: When piping stdin, commands execute before subscription data arrives
  - Alternatives considered: Channel-based signaling, sleep delay

## Final Verification

- Commands run: `make smoke`, `scripts/audit_harness.sh .`, `scripts/audit_control.sh .`
- Key outputs: All checks pass, module published to maincloud, CLI connects and sends messages
- Follow-up tasks: Write unit tests for reducers, add DM support, implement web/mobile clients
