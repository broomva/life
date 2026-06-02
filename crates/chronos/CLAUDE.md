# Chronos — Temporality Primitive for the Life Agent OS

> *Chronos* (χρόνος, Greek: time) — the substrate that answers **when an agent should wake**
> and **what it should do when it wakes**. Biological analogue: circadian rhythm.

**Status**: M1 (agenda store + HTTP wake ingest). M2–M3 planned per
`docs/superpowers/plans/2026-05-13-chronos-temporal-primitive.md` (the M1 spec also lives in
Linear BRO-1069).

## Architecture

```
chronos/
├── chronos-core/         # Pure types + traits + WakeRouter + AgendaStore (depends ONLY on aios-protocol)
├── chronos-triggers/     # HeartbeatTrigger + HttpTrigger (real) + stubs for cron/fs/sub-agent/threshold/webhook
├── chronos-lago/         # Lago bridge — record_wake + LagoAgendaStore (Custom("chronos.*") events)
├── chronos-api/          # M1 axum HTTP API — POST /v1/wake, GET /v1/agenda/{session_id}, GET /v1/health
└── chronosd/             # Daemon binary — lib + bin split, like haimad / autonomicd; --http-bind mounts the API
```

`chronos-substrate-proto` and `life-chronos` (Topology B + kernel adapter) are M3+ work and not
scaffolded yet. The M2 kernel wake handoff (`WakeEvent → tick_on_branch`) is also still planned.

## Core Types (chronos-core)

| Type | Purpose |
|------|---------|
| `WakeEvent` | `{ event_id, fired_at_unix_ms, source, payload, target_session? }` — the universal wake shape |
| `WakeEventId` | ULID newtype, sortable by creation time |
| `WakeSource` | `Heartbeat | Http | Cron | FsWatch | SubAgentReturn | Threshold | Webhook` |
| `WakeTrigger` | `async fn next_wake(&mut self) -> Option<WakeEvent>; fn name(&self) -> &'static str` |
| `WakeRouter` | Multiplexes triggers concurrently via tokio mpsc into a single stream |
| `AgendaStore` | Trait: `add` / `complete` / `defer` / `cancel` / `list(session)` — the durable agenda (M1) |
| `AgendaItem` | `{ id, session_id, intent, priority, state, source, created_at_unix_ms, not_before? }` |
| `Priority` / `AgendaItemState` | `Urgent>Normal>Deferrable` (FIFO within band); `Pending\|Completed\|Deferred\|Cancelled` |
| `InMemoryAgendaStore` | Reference `AgendaStore` for tests + the `chronos-api` suite (no lago needed) |
| `ChronosError` / `ChronosResult` | Crate-local error type (adds `NotFound` / `Agenda` in M1) |

## Event Namespace

All chronos events use `EventKind::Custom { event_type: "chronos.*", data }`, mirroring the
autonomic (`autonomic.*`) and haima (`finance.*`) convention.

| event_type | When emitted |
|------------|--------------|
| `chronos.wake` | A trigger fired and produced a `WakeEvent`. Routes to the wake's target session (M0). |
| `chronos.agenda.added` | An agenda item was created (M1). Carries the full item snapshot. |
| `chronos.agenda.completed` | An agenda item completed (M1 exposes the transition; M2 drives it from the kernel). |
| `chronos.agenda.deferred` | An agenda item was deferred to a later time (M1). Carries `not_before_unix_ms`. |
| `chronos.agenda.cancelled` | An agenda item was withdrawn before dispatch (M1). |

All `chronos.agenda.*` events land in the dedicated **`chronos.agenda`** ledger session (the item's
*target* session is a field in the payload), so `LagoAgendaStore::{complete,defer,cancel}` can
address an item by id alone. `chronos.wake` events route to their target session, separately.

The kernel does NOT yet ship a typed `EventKind::ChronosWake` variant — see the
[plan](../../docs/superpowers/plans/2026-05-13-chronos-temporal-primitive.md) §"Constraints"
for the reasoning. Promote to a typed variant after M2-M3 stabilizes the payload shape.

## Dependency Rules

```
chronos-core → aios-protocol (ONLY internal dep) + tokio, serde, ulid, thiserror, async-trait
chronos-triggers → chronos-core (no other internal deps) + tokio mpsc, async-trait
chronos-lago → chronos-core + aios-protocol + lago-core (read), lago-journal (dev only) + async-trait
chronos-api → chronos-core (ONLY internal dep) + axum, serde, tokio  [concrete store + trigger injected]
chronosd → all the above + chronos-api + lago-journal + anyhow + clap + tracing-subscriber
```

Enforced by `scripts/architecture/verify_dependencies_chronos.sh`. The CI lane runs
`cargo metadata` and walks edges; violations fail the build.

## Daemon CLI

```
chronosd --heartbeat-seconds 5 --data-dir /tmp/chronos-smoke
chronosd --http-bind 127.0.0.1:3737 --heartbeat-seconds 60 --data-dir /tmp/chronos   # M1: HTTP wake ingest
```

Flags:

| Flag | Default | Env | Purpose |
|------|---------|-----|---------|
| `--heartbeat-seconds` | 10 | `CHRONOSD_HEARTBEAT_SECONDS` | Tick interval. ≥1. |
| `--data-dir` | `/tmp/chronosd` | `CHRONOSD_DATA_DIR` | Holds the lago redb journal (created on startup). |
| `--journal-filename` | `journal.redb` | `CHRONOSD_JOURNAL_FILENAME` | File name inside `--data-dir`. |
| `--router-buffer` | 64 | `CHRONOSD_ROUTER_BUFFER` | Mpsc capacity for the router. ≥1. |
| `--http-bind` | *(unset)* | `CHRONOSD_HTTP_BIND` | M1 wake-ingest API bind addr (e.g. `127.0.0.1:3737`). Unset ⇒ heartbeat-only. |

SIGTERM and SIGINT trigger a clean shutdown that drains the router within ~2 seconds.

## Default Routing

Wakes without a `target_session` land in the **`chronos.system`** session on the **`main`**
branch. This keeps heartbeat noise out of real user sessions while remaining replayable.

Constants exposed by `chronos-lago`:

```rust
pub const CHRONOS_WAKE_EVENT_TYPE: &str = "chronos.wake";
pub const CHRONOS_SYSTEM_SESSION: &str = "chronos.system";
pub const CHRONOS_DEFAULT_BRANCH: &str = "main";
// M1 — agenda ledger:
pub const CHRONOS_AGENDA_SESSION: &str = "chronos.agenda";
pub const CHRONOS_AGENDA_ADDED_EVENT_TYPE: &str = "chronos.agenda.added";
// + chronos.agenda.{completed,deferred,cancelled}
```

## HTTP API (M1 — `chronos-api`)

Enabled with `chronosd --http-bind <addr>`. Localhost-only in M1 (auth lands when chronosd moves
behind lifegw).

| Method · Route | Body / Param | Effect |
|---|---|---|
| `POST /v1/wake` | `{ session_id?, intent, priority?, when_unix_ms?, source? }` | Adds an agenda item; fires a `chronos.wake` now unless `when_unix_ms` is in the future. → `202 { agenda_item_id, fired, session_id }` |
| `GET /v1/agenda/{session_id}` | — | Lists the session's agenda items (dispatch order), folded from the lago journal. |
| `GET /v1/health` | — | `{ "status": "ok", "service": "chronos-api" }` |

`chronos-api` holds an `Arc<dyn AgendaStore>` + an `mpsc::Sender<WakeEvent>`, both injected by
`chronosd` — it never names `chronos-lago` or `chronos-triggers` (clean dep graph).

## Commands

```bash
# Build & test the five chronos crates
cargo build  -p chronos-core -p chronos-triggers -p chronos-lago -p chronos-api -p chronosd
cargo test   -p chronos-core -p chronos-triggers -p chronos-lago -p chronos-api -p chronosd
cargo clippy -p chronos-core -p chronos-triggers -p chronos-lago -p chronos-api -p chronosd -- -D warnings

# M1 live smoke (HTTP wake ingest)
cargo run -p chronosd -- --http-bind 127.0.0.1:3737 --heartbeat-seconds 60 --data-dir /tmp/chronos-m1 &
curl -s -XPOST 127.0.0.1:3737/v1/wake -H 'content-type: application/json' -d '{"intent":"hello","session_id":"s"}'
curl -s 127.0.0.1:3737/v1/agenda/s
cargo run -p lago -- log --data-dir /tmp/chronos-m1 --session chronos.agenda   # shows chronos.agenda.added

# Run the daemon (writes one chronos.wake every 5 seconds to /tmp/chronos-smoke/journal.redb)
cargo run -p chronosd -- --heartbeat-seconds 5 --data-dir /tmp/chronos-smoke

# Replay the journal as a tree
cargo run -p lago-cli -- replay --tree --data /tmp/chronos-smoke/journal.redb
```

## Milestone Roadmap

| Milestone | Goal | Status |
|-----------|------|--------|
| M0 | Scaffold + heartbeat trigger writes `chronos.wake` events to lago | **shipped** |
| M1 | Agenda store + HTTP `POST /v1/wake` ingest | **shipped (BRO-1069)** |
| M2 | Kernel wake handoff — `WakeEvent → kernel.dispatch(session_id, intent)` | planned (BRO-1080) |
| M3 | File-watch + sub-agent return triggers | planned |
| Beyond M3 | `chronos-substrate-proto`, cron, webhook, threshold triggers, multi-priority queue | planned |

## Conventions

- **Edition**: 2024 (Rust 1.85)
- **No unsafe**: `#![forbid(unsafe_code)]` in every crate
- **Errors**: `thiserror` for libraries, `anyhow` for binaries
- **Custom prefix**: every chronos event MUST use `"chronos."` namespace
- **Module style**: `name.rs` file-based modules (not `mod.rs`)
- **Heartbeat interval**: 10s in dev, 60s+ in production — NOT 1s (floods the journal)
- **`tokio-cron-scheduler`** (when added in M3+): pin to a workspace-consistent version

## Integration Points (future)

| Crate | How Chronos integrates |
|-------|------------------------|
| **arcand** | (M2) calls `tick_on_branch(session_id, branch, TickInput { objective })` on wake fire |
| **Lago** | every wake journals as `Custom("chronos.wake")`; agenda transitions as `Custom("chronos.agenda.*")` |
| **Nous** | (M3+) threshold trigger fires when `nous_score < 0.3` |
| **Vigil** | (M2+) wake event spans carry `chronos.source` + `chronos.target_session` |
| **Spec D Anima** | (M2) wake invokes use the session's anima identity — no special handling needed |

## Constraints (carried over from the plan)

1. **Don't touch the canonical loop in `aios-runtime`.** Chronos calls `tick_on_branch`
   from outside the loop body.
2. **Use `Custom { event_type, data }` for all chronos events** until the contract stabilizes.
3. **L3 governance budget**: edits to `CLAUDE.md` / `AGENTS.md` / `METALAYER.md` /
   `.life/control/policy.yaml` count against the λ₃ budget — batch them at the end of M2.
4. **Default heartbeat**: 10s dev, 60s+ production. Never 1s.
