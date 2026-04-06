# Opsis — World State Engine

[![CI](https://github.com/broomva/opsis/actions/workflows/ci.yml/badge.svg)](https://github.com/broomva/opsis/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

AI-native continuous world state simulation for [Life Agent OS](https://github.com/broomva/life). Gives agents real-world spatial perception and action through an event-sourced geospatial world model.

## Overview

Opsis renders a real-time 3D globe with live data feeds (earthquakes, weather) streamed via SSE from a Rust backend (`opsisd`). The frontend uses CesiumJS with Google 3D Tiles for photorealistic terrain.

### Architecture

```
┌─────────────────────────────┐
│  @opsis/web (Next.js 16)    │
│  CesiumJS + Tactical HUD    │
│  ← SSE ───────────────────┐ │
└──────────────┬─────────────┘ │
               │               │
┌──────────────▼─────────────┐ │
│  opsisd (Rust)             │ │
│  Event bus + Feed registry │ │
│  /health  /stream ─────────┘ │
│  ← HTTP ──────────────────── │
│  USGS Earthquake Feed       │
│  Open-Meteo Weather Feed     │
└──────────────────────────────┘
```

## Quick Start

### Prerequisites

- [Bun](https://bun.sh) >= 1.3
- [Rust](https://rustup.rs) >= 1.93 (for the backend)
- [Cesium Ion](https://ion.cesium.com/) account (free tier)
- [Google Maps Platform](https://cloud.google.com/maps-platform) API key (for 3D Tiles)

### Frontend

```bash
cd apps/web
cp .env.example .env.local
# Edit .env.local with your API keys

bun install
bun run dev          # → http://localhost:3020
```

### Backend

```bash
# In the Life Agent OS repo (github.com/broomva/life)
cargo run -p opsisd  # → http://localhost:3010
```

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `NEXT_PUBLIC_CESIUM_ION_TOKEN` | Yes | Cesium Ion access token |
| `NEXT_PUBLIC_GOOGLE_MAPS_API_KEY` | Yes | Google 3D Tiles API key |
| `OPSIS_URL` | No | Backend URL (default: `http://localhost:3010`) |

## Project Structure

```
opsis/
├── apps/web/              # Next.js 16 frontend
│   ├── app/               # App router pages
│   └── public/            # CesiumJS static assets
├── packages/opsis-core/   # Shared components, hooks, types
│   ├── src/components/    # Globe, HUD, overlays
│   ├── src/hooks/         # useOpsisStream, useCesium
│   └── src/lib/           # Utilities, types
└── docs/                  # Architecture & specs
```

## Tech Stack

- **Runtime**: [Bun](https://bun.sh) 1.3
- **Framework**: [Next.js](https://nextjs.org) 16 with Turbopack
- **3D Globe**: [CesiumJS](https://cesium.com) + [Resium](https://resium.reearth.io)
- **Styling**: [Tailwind CSS](https://tailwindcss.com) v4 + Arcan Glass
- **Linting**: [Biome](https://biomejs.dev) 2.0
- **Monorepo**: [Turborepo](https://turbo.build)
- **Backend**: Rust ([opsisd](https://github.com/broomva/life)) with Axum + SSE

## Development

```bash
bun run dev            # Start dev server
bun run build          # Production build
bun run lint           # Biome lint + format check
bun run check-types    # TypeScript type checking
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

[MIT](LICENSE) — Broomva Tech
