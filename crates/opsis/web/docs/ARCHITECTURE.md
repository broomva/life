# Opsis Architecture

**Version:** 0.1.0 | **Date:** 2026-04-06

Opsis is an AI-native continuous world state engine for the Life Agent OS. It ingests real-world data feeds, aggregates them into domain-specific state lines, and streams the result to connected clients via SSE.

## System Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     EXTERNAL DATA SOURCES                        │
│  USGS Earthquakes ─────┐                                         │
│  Open-Meteo Weather ───┤  (future: ADS-B, CelesTrak, markets,   │
│  (pluggable feeds) ────┘   news, Garmin, infrastructure...)      │
└──────────────────┬──────────────────────────────────────────────┘
                   │ HTTP polling (30s–300s intervals)
                   ▼
┌─────────────────────────────────────────────────────────────────┐
│  opsisd — Rust Daemon (localhost:3010)                            │
│                                                                   │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────────┐       │
│  │ Feed Tasks   │  │ Event Bus    │  │ Tick Aggregator   │       │
│  │ (tokio::spawn│  │ (broadcast)  │  │ (1 Hz EMA)        │       │
│  │  per feed)   │→ │ 16K capacity │→ │ drain → flush     │       │
│  └─────────────┘  └──────────────┘  └───────┬───────────┘       │
│                                              │                    │
│                                     ┌────────▼────────┐          │
│                                     │ WorldDelta      │          │
│                                     │ (per-tick SSE)  │          │
│                                     └────────┬────────┘          │
│                                              │                    │
│  ┌──────────┐                       ┌────────▼────────┐          │
│  │ /health  │                       │ /stream (SSE)   │          │
│  │ (axum)   │                       │ (axum SSE)      │          │
│  └──────────┘                       └─────────────────┘          │
└─────────────────────────────────────────────────────────────────┘
                   │ EventSource (SSE over HTTP)
                   ▼
┌─────────────────────────────────────────────────────────────────┐
│  @opsis/web — Next.js 16 (localhost:3020)                        │
│                                                                   │
│  ┌─────────────────┐  ┌──────────────┐  ┌──────────────────┐   │
│  │ useOpsisStream   │  │ Globe        │  │ StatePanel       │   │
│  │ (React hook,     │→ │ (CesiumJS    │  │ FeedPanel        │   │
│  │  EventSource)    │  │  3D globe)   │  │ ConnectionStatus │   │
│  └─────────────────┘  └──────────────┘  └──────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## Repositories

| Repo | Location | Purpose |
|------|----------|---------|
| broomva/life | `core/life/crates/opsis/` | Rust backend (opsis-core, opsis-engine, opsisd) |
| broomva/opsis | `apps/opsis/` | Web UI (Next.js + CesiumJS) |

## Rust Backend

### Crate Dependency Graph

```
aios-protocol (Life Agent OS canonical contract — unchanged)
    │
    ▼
opsis-core (types, traits, zero IO)
    │   GeoPoint, Bbox, GeoHotspot, WorldClock, WorldTick,
    │   StateDomain, StateLine, Trend, WorldState,
    │   RawFeedEvent, StateEvent, WorldDelta, StateLineDelta,
    │   FeedIngestor trait, FeedSource, SchemaKey,
    │   Subscription, ClientId, OpsisError
    │
    ▼
opsis-engine (runtime — clock, bus, feeds, aggregation, streaming)
    │   OpsisEngine, EngineConfig, EventBus, TickAggregator,
    │   ClientRegistry, SSE stream server (axum),
    │   UsgsEarthquakeFeed, OpenMeteoWeatherFeed
    │
    ▼
opsisd (binary daemon — CLI, startup, shutdown)
```

### Feed Pipeline (Data Flow)

```
1. POLL       feed.poll_raw()        → Vec<RawFeedEvent>
                                       (raw JSON from external API)

2. NORMALIZE  feed.normalize(raw)    → Vec<StateEvent>
                                       (domain, severity, location, summary)

3. PUBLISH    bus.publish_event(e)   → broadcast channel (16K)
                                       if severity >= 0.8 → fast_path channel too

4. DRAIN      event_rx.try_recv()    → aggregator.push(event)
              (tick loop, 1 Hz)

5. FLUSH      aggregator.flush()     → WorldDelta
              - Group by StateDomain
              - EMA smoothing (α=0.3) on activity per domain
              - Trend detection (Spike/Crash/Rising/Falling/Stable)
              - Spatial clustering of event locations (greedy, 50km eps)
              - Top-10 events by severity per domain

6. BROADCAST  bus.publish_delta()    → SSE subscribers receive WorldDelta
```

### Two-Tier Event Model

Every feed produces events in two tiers:

**Tier 1 — RawFeedEvent** (preserved as-is):
```rust
pub struct RawFeedEvent {
    pub id: EventId,           // ULID
    pub timestamp: DateTime<Utc>,
    pub source: FeedSource,    // "usgs-earthquake"
    pub feed_schema: SchemaKey, // "usgs.geojson.v1"
    pub location: Option<GeoPoint>,
    pub payload: serde_json::Value,  // original JSON, untouched
}
```

**Tier 2 — StateEvent** (normalized, drives state lines):
```rust
pub struct StateEvent {
    pub id: EventId,
    pub tick: WorldTick,
    pub domain: StateDomain,     // Emergency, Weather, Finance, etc.
    pub location: Option<GeoPoint>,
    pub severity: f32,           // 0.0–1.0
    pub summary: String,         // "M2.5 earthquake — 84 km SE of Chignik, Alaska"
    pub source: FeedSource,
    pub tags: Vec<String>,
    pub raw_ref: EventId,        // pointer back to RawFeedEvent
}
```

One raw event can produce multiple state events (e.g., a news article about an earthquake near a port → Emergency + Trade disruption).

### State Lines

12 built-in domains + arbitrary Custom domains:

| Domain | Description | Current Feeds |
|--------|-------------|---------------|
| Emergency | Earthquakes, disasters | USGS |
| Weather | Storms, anomalies | Open-Meteo |
| Health | Pandemics, outbreaks | — |
| Finance | Markets, crypto | — |
| Trade | Shipping, logistics | — |
| Conflict | Military activity | — |
| Politics | Elections, diplomacy | — |
| Space | Satellites, launches | — |
| Ocean | Maritime, eDNA | — |
| Technology | Outages, launches | — |
| Personal | Health, calendar | — |
| Infrastructure | Servers, deploys | — |

Each state line tracks:
- **activity** (f32, 0.0–1.0): exponential moving average, α=0.3
- **trend** (Spike/Crash/Rising/Falling/Stable): derivative over last 3 samples
- **hotspots** (Vec<GeoHotspot>): spatial clusters of events
- **recentEvents** (last 50): for the feed panel

### Event Bus (Broadcast Channels)

Three independent tokio::sync::broadcast channels:

| Channel | Capacity | Purpose |
|---------|----------|---------|
| event_tx | 16,384 | All normalized StateEvents |
| delta_tx | 256 | Per-tick WorldDelta (SSE consumers) |
| fast_path_tx | 1,024 | High-severity events (≥0.8) |

**Backpressure:** Slow receivers get `Lagged(n)` error and skip missed events.

### World Clock

- **Base tick:** 1 Hz (configurable via `--hz`)
- **Monotonic counter:** WorldTick(u64), never resets
- **Time scale:** 1.0 for main universe (future: Nx for simulations)

## Adding a New Feed

Implement the `FeedIngestor` trait:

```rust
pub trait FeedIngestor: Send + Sync {
    fn source(&self) -> FeedSource;
    fn schema(&self) -> SchemaKey;
    fn connect(&self) -> Pin<Box<dyn Future<Output = OpsisResult<()>> + Send + '_>>;
    fn poll_raw(&self) -> Pin<Box<dyn Future<Output = OpsisResult<Vec<RawFeedEvent>>> + Send + '_>>;
    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<StateEvent>>;
    fn poll_interval(&self) -> Duration;
}
```

Then register it in `opsisd/src/main.rs`:

```rust
engine.add_feed(Box::new(MyCustomFeed::new()));
```

### Example: Adding an ADS-B Flight Feed

```rust
pub struct AdsbFeed {
    client: reqwest::Client,
}

impl FeedIngestor for AdsbFeed {
    fn source(&self) -> FeedSource { FeedSource::new("adsb-exchange") }
    fn schema(&self) -> SchemaKey { SchemaKey::new("adsb.v2") }

    fn poll_raw(&self) -> Pin<Box<...>> {
        Box::pin(async {
            let resp = self.client.get("https://...").send().await?;
            let data: Value = resp.json().await?;
            // Parse aircraft positions into RawFeedEvents
            Ok(events)
        })
    }

    fn normalize(&self, raw: &RawFeedEvent) -> OpsisResult<Vec<StateEvent>> {
        // Map aircraft → Trade domain (cargo) or Space domain (military)
        Ok(vec![StateEvent { domain: StateDomain::Trade, ... }])
    }

    fn poll_interval(&self) -> Duration { Duration::from_secs(5) }
}
```

## Data Storage (Current State)

**Phase 1 is in-memory only.** No persistence across restarts.

| Tier | Implementation | Retention |
|------|---------------|-----------|
| Hot | In-memory WorldState + event buffers | Current session |
| Warm | — (planned: Lago journal) | — |
| Cold | — (planned: Parquet files) | — |

### Planned Storage Architecture (Phase 3+)

```
HOT  (last 1h)  → Apache Arrow RecordBatches, queryable via DataFusion
WARM (last 7d)  → Lago journal (redb, append-only, event-sourced)
COLD (90d+)     → Parquet files, partitioned by date+domain
```

## Web Frontend

### Tech Stack

| Layer | Technology |
|-------|-----------|
| Framework | Next.js 16 + React 19 |
| 3D Globe | CesiumJS (Cesium Ion + Google 3D Tiles) |
| Styling | Tailwind v4 + custom Arcan Glass CSS |
| Package Manager | Bun |
| Linter | Biome |
| Monorepo | Turborepo |

### Package Structure

```
apps/opsis/
├── packages/opsis-core/     ← Shared React components + hooks + types
│   ├── src/lib/types.ts     ← TypeScript types (mirrors Rust opsis-core)
│   ├── src/lib/utils.ts     ← cn(), activityColor(), trendIndicator()
│   ├── src/hooks/
│   │   └── use-opsis-stream.ts  ← SSE EventSource hook with auto-reconnect
│   └── src/components/
│       ├── globe.tsx         ← CesiumJS 3D globe (with CSS fallback)
│       ├── state-panel.tsx   ← Domain activity levels + trends
│       ├── feed-panel.tsx    ← Tabbed event feed with severity dots
│       ├── timeline.tsx      ← Canvas waveform renderer per domain
│       └── connection-status.tsx
│
└── apps/web/                ← Next.js shell
    ├── app/page.tsx         ← Main command center layout
    ├── app/globals.css      ← Arcan Glass + Tactical design system
    └── .env.local           ← NEXT_PUBLIC_GOOGLE_MAPS_API_KEY, NEXT_PUBLIC_CESIUM_ION_TOKEN
```

### SSE Connection Flow

```
Browser loads page
    → useOpsisStream() hook creates EventSource("http://localhost:3010/stream")
    → On each "world_delta" SSE event:
        1. Parse JSON → WorldDelta
        2. Update worldState.stateLines (Map<domain, StateLine>)
        3. Append new events to worldState.allEvents (capped at 500)
        4. React re-renders: globe markers, state panel, feed panel, ticker
    → On "lagged" event: show warning
    → On error: auto-reconnect after 3s
```

### Design System

**Arcan Glass + Tactical HUD:**
- Dark space background (oklch 0.06-0.10)
- Liquid glass panels: `backdrop-filter: blur(24px) saturate(1.4)`
- Cyan accent (#06b6d4 range) with glow effects
- Monospace font (SF Mono / JetBrains Mono)
- Severity color coding: red (critical) → amber (high) → green (low)
- Corner bracket decorations (WorldView style)
- Category pills for feed filtering

## Running

```bash
# Terminal 1: Rust backend
cd ~/broomva/core/life
git checkout feature/bro-460-opsis-phase1-foundation
cargo run -p opsisd              # Starts at localhost:3010

# Terminal 2: Web frontend
cd ~/broomva/apps/opsis
bun install                       # First time only
bun run dev                       # Starts at localhost:3020
```

### Environment Variables

| Variable | Required | Purpose |
|----------|----------|---------|
| NEXT_PUBLIC_GOOGLE_MAPS_API_KEY | No | Google Photorealistic 3D Tiles |
| NEXT_PUBLIC_CESIUM_ION_TOKEN | No | Cesium Ion base imagery |

## API Endpoints

### opsisd (localhost:3010)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | JSON health check (uptime, client count) |
| `/stream` | GET | SSE stream of WorldDelta events (1 Hz) |

### SSE Event Types

| Event | Data | Frequency |
|-------|------|-----------|
| `world_delta` | JSON WorldDelta | Every tick (1 Hz) |
| `lagged` | Warning message | When client falls behind |

## Testing

```bash
# Rust tests (34 total)
cd ~/broomva/core/life
cargo test -p opsis-core          # 22 tests
cargo test -p opsis-engine        # 12 tests

# Web build validation
cd ~/broomva/apps/opsis
bun run build                     # Next.js production build
```

## Linear Project

[Opsis — World State Engine](https://linear.app/broomva/project/opsis-world-state-engine-ab1679c83fcc)

| Phase | Epic | Status |
|-------|------|--------|
| 1A: Rust Engine | BRO-460 | Done (PR #531) |
| 1B: Web UI | — | Done (broomva/opsis repo) |
| 2: Agent Integration | BRO-461 | Planned |
| 3: Feeds + Timeline | BRO-462 | Planned |
| 4: Universe Branching | BRO-463 | Planned |
| 5: Newton Physics | BRO-464 | Planned |
| 6: Desktop + Polish | BRO-465 | Planned |
