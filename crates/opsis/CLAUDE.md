# Opsis — World State Engine

AI-native continuous world state simulation for Life Agent OS. Gives agents real-world spatial perception and action through an event-sourced geospatial world model.

## Quick Start

```bash
# Backend (Rust) — from the Life monorepo root
cargo run -p opsisd              # localhost:3010

# Frontend (Next.js) — from crates/opsis/web/
cd web && bun install && bun run dev       # localhost:3020
```

## Structure

```
crates/opsis/
├── opsis-core/        ← Rust types, spatial primitives, event definitions
├── opsis-engine/      ← Rust event bus, feed registry, SSE streaming
├── opsisd/            ← Rust daemon (axum HTTP server)
└── web/               ← Next.js 16 frontend (Turborepo monorepo)
    ├── apps/web/      ← App router + CesiumJS 3D globe
    └── packages/opsis-core/  ← Shared React components, hooks, types
```

## Commands

```bash
# Rust
cargo run -p opsisd          # Run backend daemon
cargo test -p opsis-core     # Test Rust crate
cargo test -p opsis-engine   # Test engine

# Frontend (from web/)
bun run dev            # Start dev server (port 3020)
bun run build          # Production build
bun run lint           # Biome check
bun run check-types    # TypeScript check
```

## Stack

- **Rust** + Axum for backend (opsisd)
- **Bun** package manager (not npm/yarn)
- **Biome** linter (not ESLint)
- **Tailwind v4** with Arcan Glass + Tactical CSS
- **CesiumJS** for 3D globe (Cesium Ion + Google 3D Tiles)
- **Next.js 16** with Turbopack
- **React 19**
- **TypeScript** strict mode

## Conventions

- Monospace font throughout (SF Mono / JetBrains Mono)
- Dark space background with liquid glass panels
- Cyan accent with glow effects for tactical HUD
- Severity: red (critical) → amber (high) → green (low)
- SSE connection to opsisd backend at localhost:3010

## Env Vars

Set in `web/apps/web/.env.local`:
- `NEXT_PUBLIC_GOOGLE_MAPS_API_KEY` — Google 3D Tiles
- `NEXT_PUBLIC_CESIUM_ION_TOKEN` — Cesium base imagery

## Deployment

- **Frontend**: Vercel — deploys from `broomva/opsis` mirror repo, root directory `web/`
- **Backend**: Railway — deploys from `broomva/life`, Dockerfile at `deploy/railway/Dockerfile.opsisd`
- **Mirror**: splitsh-lite syncs `crates/opsis/` → `broomva/opsis` on every push to main

## Related

- **Design spec:** `~/broomva/docs/superpowers/specs/2026-04-05-opsis-design.md`
- **Linear:** Opsis — World State Engine project
