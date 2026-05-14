# Agent-driven feedback loop — Lumen Phase α

**Status**: Active scaffold. The agent can now empirically validate
changes to lifegw / lifed / lumen by actually exercising the warp-oss
GUI client, not just the wire-protocol surface.

## Why this exists

Before this scaffold, Phase α validation had two tiers:

1. **Layer 1 — curl smoke** (`scripts/lumen-smoke.sh smoke-curl`) — three
   baseline HTTPS gates. Proves lifegw accepts requests but says nothing
   about whether the actual GUI app works.
2. **Layer 2-headless — `lumen_smoke_client`** — five wire-protocol
   gates including a live WSS subscription. Proves the *wire* matches
   what warp-oss would send.

Neither answered the question: **does the actual user-facing GUI
behave correctly when wired to Life?** That gate is what the
`scripts/lumen-app.sh` toolkit closes.

A human can answer it by launching the app and clicking around. An
agent making changes to lifegw resolvers, lifed dispatch, or the
warp-oss patch needs the same loop — launch, drive UI, capture state,
diff against baseline.

## Components

```
┌─────────────────────────────────────────────────────────────────┐
│                  Agent feedback loop (this file)                 │
└─────────────────────────────────────────────────────────────────┘

  Edit lifegw/lifed/lumen code
            │
            ▼
  ┌────────────────────────────────────────────────────────────┐
  │  scripts/lumen-smoke.sh up                                 │  Backend
  │  ├── lifed  (mock substrates, UDS @ /tmp/lumen-smoke/...)  │
  │  └── lifegw (HTTPS+WSS on 127.0.0.1:8443, dev-tls cert)    │
  └────────────────────────────────────────────────────────────┘
            │
            ▼
  ┌────────────────────────────────────────────────────────────┐
  │  scripts/lumen-app.sh build      (patched warp-oss binary) │  Build
  │  ├── apply channel-oss-override.patch                      │
  │  ├── apply skip-metal-when-wgpu.patch                      │
  │  └── cargo build --release --bin warp-oss [--features ..]  │
  └────────────────────────────────────────────────────────────┘
            │
            ▼
  ┌────────────────────────────────────────────────────────────┐
  │  scripts/lumen-app.sh launch                               │  Drive
  │  ├── spawn warp-oss with WARP_*_URL env vars               │
  │  ├── osascript activate / keystroke / key code             │
  │  ├── screencapture -l <window-id>                          │
  │  └── log tail (lifegw / lifed / warp-oss interleaved)      │
  └────────────────────────────────────────────────────────────┘
            │
            ▼
  ┌────────────────────────────────────────────────────────────┐
  │  Evidence: docs/lumen/validation/YYYY-MM-DD/                │  Capture
  │  ├── screenshot-HHMMSS.png    per-state captures           │
  │  ├── lifegw.log               server-side trace            │
  │  ├── warp-oss.log             client-side stderr           │
  │  └── README.md                gate ledger + verdict        │
  └────────────────────────────────────────────────────────────┘
            │
            ▼
  Decide: change is good → commit, OR loop back to edit
```

## The toolkit subcommands

`scripts/lumen-app.sh` — every primitive the agent needs:

| Command | Purpose |
|---|---|
| `build` | Apply patches + `cargo build --release` warp-oss with Metal |
| `build --wgpu` | Same, but with `experimental-wgpu-renderer` (Metal-bypass) |
| `launch` | Spawn warp-oss with Life env vars wired up |
| `pid` | Show running warp-oss PID |
| `windows` | Enumerate warp-oss window IDs + titles via System Events |
| `shot [PATH]` | Screencapture the front window to PATH |
| `type "TEXT"` | Keystroke TEXT into the focused app |
| `key NAME` | Send a named key (return, tab, escape, arrows, etc.) |
| `chord "cmd+k"` | Send a modified key combo |
| `click X Y` | Click at absolute screen coords |
| `logs tail` | Unified follow of lifegw + lifed + warp-oss logs |
| `logs grep PAT` | Search across the three logs |
| `wait "TEXT"` | Block until TEXT appears in lifegw.log (signal-based wait) |
| `probe` | Quick health check — PIDs, windows, last log line |
| `loop` | Orchestrated end-to-end demo (launch → drive → screenshot) |
| `down` | Stop warp-oss cleanly |

## Why osascript over MCP browser tools

The agent-browser / gstack skills only drive web browsers via CDP.
warp-oss is a **native AppKit + Metal** app — there's no embedded
browser to control. osascript talks directly to macOS Accessibility
APIs, which is the canonical macOS-native automation surface. It's
also fully built in to the OS (no Homebrew prerequisites).

For richer automation (sub-pixel coords, modifier-chord precision)
the toolkit auto-detects `cliclick` and uses it when present.

## Validation cadence

After any non-trivial change to lifegw/lifed resolvers:

```bash
# 1. Bring stack back up cleanly.
./scripts/lumen-smoke.sh down && ./scripts/lumen-smoke.sh up

# 2. Confirm wire-protocol surface still green (cheap, ~5s).
cargo run --example lumen_smoke_client -p lifegw

# 3. Drive the real client (expensive, ~30s + UI render time).
./scripts/lumen-app.sh down                    # ensure clean state
./scripts/lumen-app.sh launch                  # spawn the app
./scripts/lumen-app.sh shot docs/lumen/validation/$(date -u +%F)/post-change.png
./scripts/lumen-app.sh logs grep "createAgentTask"   # confirm app dispatched
./scripts/lumen-app.sh down

# 4. Diff the screenshot against the previous good baseline.
git diff -- docs/lumen/validation/$(date -u +%F)/post-change.png
```

Steps 1+2 catch wire-level regressions. Steps 3+4 catch behavioural
regressions the wire couldn't see (e.g. the app renders an error
toast instead of streaming the response).

## Anti-patterns the toolkit guards against

- **"It compiled, ship it"** — compilation is a necessary condition
  for correctness, not sufficient. `lumen-app.sh loop` adds the
  empirical-feedback gate (P11) on top of cargo's static checks.
- **"The wire-protocol smoke passed"** — also necessary, not
  sufficient. The smoke proves the bytes match; the UI run proves
  the bytes mean what we think they mean.
- **"I'll launch it manually and check"** — manual workflows
  decay. A scripted feedback loop runs unattended; manual flows get
  skipped under deadline pressure.

## Composing with bstack primitives

| Primitive | Role in this loop |
|---|---|
| P11 — Empirical | This is the canonical instantiation for Phase α |
| P15 — Snapshot | `lumen-app.sh probe` reports current state cheaply |
| P14 — Dep-Chain | The toolkit makes downstream consumers concrete |
| P7  — Wait | `lumen-app.sh wait "TEXT"` replaces sleep loops on signals |
| P6  — Bookkeeping | Evidence under `docs/lumen/validation/` flows into the KG |
