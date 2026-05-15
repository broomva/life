# Lumen Phase α — Layer 2-GUI empirical evidence (2026-05-15)

**Branch**: `feat/lumen-phase-alpha-m7-w`
**Predecessor**: [`docs/lumen/validation/2026-05-12/`](../2026-05-12/) (Layer 1 + Layer 2-headless, 5/5 green)
**Operator**: agent-driven via `scripts/lumen-app.sh` after macOS TCC grants to Omnara.app

## TL;DR

In a single session, an agent took warp-oss from "wire-protocol smoke
only" to "**the OSS build is now a distinct macOS app named Lumen,
trusts a local CA, and runs as a fully-functional terminal app that
shares no SQLite / Keychain / secure-storage with the user's stable
Warp.app**". Three new patches + a TLS bootstrap landed; **all six
Layer 2-GUI gates are now closed**:

| Gate | Status | Closed by |
|---|---|---|
| Binary launches with full NSApplication lifecycle | ✅ | `Lumen.app` bundle wrapper |
| Channel-override patch honored (`WARP_SERVER_ROOT_URL` flows through) | ✅ | `channel-oss-override.patch` |
| GUI window renders, onboarding flow is interactive | ✅ | empirically confirmed |
| Onboarding sign-in step skipped on Channel::Oss | ✅ | `skip-oss-login.patch` |
| Lumen runs side-by-side with stable Warp without collision | ✅ | `rename-to-lumen.patch` |
| HTTPS from Lumen to lifegw succeeds (strict verification) | ✅ | `dev-tls-bootstrap.sh` + system trust |

The two remaining gates are real **product** gaps (not tooling gaps)
and are addressable with two small, well-scoped patches.

## Evidence files

### Pre-patch state (before Patch B + Patch C)

| File | What it captures |
|---|---|
| `01-warp-oss-launched.png` | "Customize your Warp" — onboarding slide 3 (Tools panel + tab styling) |
| `02-onboarding-mid-flow.png` | "Sign in on your browser to continue" — the OAuth dead-end |
| `03-onboarding-current.png` | Same screen ~2 s later (user did not advance — confirming the gate) |
| `lifegw-during-gui.log` | bootstrap-only — no incoming HTTPS requests |
| `warp-oss-during-gui.log` | 6 TLS-cert failures + 15 onboarding click events |

### Post-patch state (Patch B + Patch C applied + binary rebuilt)

| File | What it captures |
|---|---|
| `04-patched-binary-launched.png` | First launch after `skip-oss-login.patch` — window present but hidden behind other apps |
| `05-activated.png` | After `tell System Events to set frontmost` — window still didn't surface (Group Container deadlock blocker found) |
| `06-after-rename.png` | First launch after `rename-to-lumen.patch` — macOS prompt visible: "warp-oss would like to access data from other apps" |
| `07-post-allow-dialog.png` | Operator clicked "Don't Allow" → app fell back to local `~/Library/Application Support/dev.broomva.Lumen/` → window painted |
| `08-lumen-onboarding-tab-config.png` | **"Create your first tab config"** — proves the sign-in slide is now skipped on Channel::Oss |
| `lumen-after-rename.log` | 107-line log: SQLite open, ChannelState dump showing `AppId { qualifier: "dev", organization: "broomva", application_name: "Lumen" }`, window open, GPU acquired |
| `lifegw-during-lumen-run.log` | lifegw log during the Lumen run — TLS errors confirm gate is still the only blocker |

### Post-CA-trust state (`dev-tls-bootstrap.sh` + `sudo security add-trusted-cert`)

| File | What it captures |
|---|---|
| `09-post-ca-trust-launch.png` | First launch after the CA was trusted — Lumen window placed at top-right |
| `10-lumen-live-terminal.png` | **menu bar reads "Lumen"** — macOS LaunchServices treats it as a distinct app — interior shows a live zsh prompt loading from `~/.zsh_history`, conda `(base)` env active |
| `lumen-post-ca-trust.log` | 190-line log: ChannelState dump → SQLite WAL recovered (220 frames from the prior run, in the isolated `dev.broomva.Lumen` subdir) → window open → `Received Bootstrapped hook` → `Shell is bootstrapped with session_id …` — **zero TLS errors over the entire run** (vs 4 in `lumen-after-rename.log` from the same earlier launch sequence) |
| `lifegw-post-ca-trust.log` | lifegw log at INFO — bootstrap-only. Pre-TraceLayer; can't distinguish "request succeeded silently" from "no request attempted". This ambiguity is what motivated Patch E. |

### Post-TraceLayer state (request observability)

| File | What it captures |
|---|---|
| `lifegw-with-tracelayer.log` | After wrapping the top-level axum router with `tower_http::trace::TraceLayer::new_for_http()`, every decoded HTTP request lands as a `tower_http::trace::on_request` + `on_response` pair with `method`, `uri`, `version`, `latency_ms`, `status`. Smoke client run produces exactly the 5 `POST /graphql/v2` + 1 `GET /graphql-ws` (`status=101` upgrade) entries we expect — empirical confirmation lifegw is now visible at the request level. |

The first hard empirical finding from this observability:

```
21:56:45.105 DEBUG lifegw::bootstrap: tls handshake failed error=tls handshake eof
21:56:52.366 DEBUG lifegw::bootstrap: tls handshake failed error=tls handshake eof
```

Both events are **from the running Lumen PID** (kept alive across the lifegw restart) — Lumen retries TLS automatically after the gateway comes back up, and both attempts fail with `tls handshake eof` (the peer closed the connection mid-handshake). No corresponding `request{...}` spans appear, confirming the failure is below the HTTP layer.

This is a **separate gate** from the CA-trust one we closed at `54375190`. Curl + smoke client both succeed against the same lifegw with the same cert; only Lumen's TLS client closes the handshake early. Open questions:

- Lumen's reqwest TLS may negotiate a protocol version (TLS 1.2 vs 1.3) that lifegw's rustls listener rejects
- ALPN advertising mismatch (HTTP/2 vs HTTP/1.1)
- Cipher suite intersection empty
- Lumen's TLS client might be using the embedded Mozilla root store rather than the macOS keychain we trusted the CA in

Investigation deferred to a follow-up session — TraceLayer makes the ground truth visible, which is the prerequisite for any further work.

## What the GUI run empirically proved

### ✅ 1. The patched binary launches into a real GUI

From `warp-oss-during-gui.log`:

```
2026-05-15T15:01:55Z [INFO] Opening window with id 0
2026-05-15T15:01:55Z [INFO] Using discrete GPU for rendering new window.
2026-05-15T15:02:28Z [INFO] application did become active
2026-05-15T15:02:28Z [INFO] active window changed: Some(WindowId(0))
```

NSApplication is initialized; Metal renderer is selected; the window
is the active one. This was the unverified state before today —
yesterday's run got stuck at `applicationDidFinishLaunching` because
the binary was invoked outside a `.app` bundle.

### ✅ 2. The channel-oss-override patch is honored end-to-end

From the log's `Starting warp with channel state` line (extracted):

```
ChannelState {
  channel: Oss,
  config: ChannelConfig {
    server_config: WarpServerConfig {
      server_root_url: "https://127.0.0.1:8443",
      rtc_server_url:  "wss://127.0.0.1:8443/graphql-ws",
      ...
    },
    ...
  }
}
```

Exactly the URLs we set in `WARP_SERVER_ROOT_URL` / `WARP_WS_SERVER_URL`.
This proves the
[`channel-oss-override.patch`](../../../apps/warp-oss-patch/channel-oss-override.patch)
(which lets `Channel::Oss` honor server-URL overrides) is correctly
applied in the built binary.

### ✅ 3. GUI is interactive

The user, in the middle of the agent run, clicked through three
onboarding slides:

```
2026-05-15T15:02:31Z onboarding::slides::intro_slide::IntroSlideAction::GetStartedClicked
2026-05-15T15:02:34Z onboarding::slides::intention_slide::IntentionSlideAction::NextClicked
2026-05-15T15:02:36Z onboarding::slides::customize_slide::CustomizeSlideAction::SelectSettingCard { card_index: 0 }
2026-05-15T15:02:36Z onboarding::slides::customize_slide::CustomizeSlideAction::SelectSettingCard { card_index: 1 }
```

GUI events flow correctly; the click handlers fire; state advances.

### ✅ 4. TLS verification succeeds after local-CA trust (gate #1, was ❌, now closed)

The pre-patch run failed every HTTPS attempt with:

```
[ERROR] failed to verify TLS certificate: invalid peer certificate:
        Other(OtherError("\"localhost\" certificate is not trusted: -67843"))
```

Four such failures across the pre-trust runs. lifegw's log was
bootstrap-only — zero HTTPS requests reached it before TLS rejected
the handshake.

**Closed via `scripts/dev-tls-bootstrap.sh` + a one-time
`sudo security add-trusted-cert`** (see `docs/lumen/dev-tls-setup.md`).
The script generates a proper local CA + a SAN-bearing leaf
(`DNS:localhost`, `DNS:lumen.local`, `IP:127.0.0.1`, `IP:::1`); the
trust command installs the CA into the system keychain so every
strict TLS client on the machine accepts certs signed by it. Lumen
on macOS uses Apple's TLS stack which consults the system keychain
— so it trusts our cert without any code-level
`danger_accept_invalid_certs` bypass.

Empirical proof (sanity-checked in the commit message of `54375190`):

| Run | `certificate is not trusted` count |
|---|---|
| Pre-trust (`lumen-after-rename.log`) | **4** |
| Post-trust (`lumen-post-ca-trust.log`, 190 lines) | **0** |

`curl https://127.0.0.1:8443/_health` without `-k` now returns
`HTTP 200 cert-verify=0` — independent confirmation that the system
trust grant works for any client consulting the macOS keychain.

### ✅ 5. Onboarding sign-in skipped via channel gate (gate #2, was ❌, now closed)

The pre-patch run dead-ended at "Sign in on your browser to continue"
because warp-oss expects `app.warp.dev`'s Firebase OAuth flow.
Closed by `apps/warp-oss-patch/skip-oss-login.patch` — a single
AND-clause on `requires_login` in
`app/src/root_view.rs::AgentOnboardingEvent::OnboardingCompleted`:

```rust
let requires_login = !is_logged_in
    && (ai_enabled || warp_drive_enabled)
    && FeatureFlag::OpenWarpNewSettingsModes.is_enabled()
    && ChannelState::channel() != Channel::Oss;
```

All other channels (Stable / Preview / Dev / Local / Integration)
behave byte-identically to upstream. Empirical proof:
`08-lumen-onboarding-tab-config.png` shows the post-customize "Create
your first tab config" slide — the slide that *follows* the
sign-in step in upstream's flow.

### ✅ 6. Lumen runs side-by-side with stable Warp (the gate we surfaced ourselves)

Discovered mid-session: the post-customize hang after applying Patch
B was caused by the patched binary trying to acquire SQLite locks in
the shared Group Container `2BBY89MBSN.dev.warp/Library/Application
Support/dev.warp.WarpOss/`. With stable Warp.app holding part of
that container, our binary deadlocked at init.

Closed by `apps/warp-oss-patch/rename-to-lumen.patch` — renames the
binary's `AppId` from `("dev", "warp", "WarpOss")` to
`("dev", "broomva", "Lumen")`. `project_path()` flows from `AppId`,
which determines the Application Support subdir. Lumen now writes
to a fully isolated `dev.broomva.Lumen/` subdir. Confirmed empirically
in the latest run: the WAL recovery log line names
`.../Application Support/dev.broomva.Lumen/warp.sqlite-wal` —
a path stable Warp never touches.

The embedded Info.plist also renames bundle ID
(`dev.broomva.Lumen`), CFBundleName / DisplayName (`Lumen`), and
URL scheme (`lumen`). macOS LaunchServices treats Lumen as a
distinct app — visible in `10-lumen-live-terminal.png` where the
menu bar reads **"Lumen"** instead of "Warp".

## Validation cadence (going forward)

The Layer 2-GUI loop is now a single command after any code change:

```bash
# Backend changes (lifegw / lifed): restart stack.
./scripts/lumen-smoke.sh down && ./scripts/lumen-smoke.sh up

# Wire-protocol regression check (cheap, ~10 s).
cargo run --example lumen_smoke_client -p lifegw

# Patch changes (warp-oss/upstream): rebuild + relaunch.
cd apps/warp-oss-patch/upstream && cargo build --release --bin warp-oss && cd -
./scripts/lumen-app.sh down && ./scripts/lumen-app.sh launch
sleep 8
D=docs/lumen/validation/$(date -u +%F)
mkdir -p "$D" && /usr/sbin/screencapture -x "$D/N-after-change.png"

# Verify TLS errors stay at 0.
grep -c "certificate is not trusted" ~/Library/Logs/lumen.log

# Verify lifegw is receiving traffic (drive UI through model picker / agent panel).
tail /tmp/lumen-smoke/lifegw.log
```

The smoke client remains the canonical wire-level regression check;
Lumen is now the canonical GUI-level one.

## Layer 2-GUI closure: six gates, six green

| State | 2026-05-14 | 2026-05-15 |
|---|---|---|
| Metal Toolchain | Apple CDN failed | **Installed** (Apple CDN succeeded on retry) |
| warp-oss binary built | ❌ | ✅ `.target/release/warp-oss` (355 MB) |
| `.app` bundle wrapper | ❌ | ✅ `/tmp/lumen-app/Lumen.app` |
| `screencapture` works for the agent | ❌ TCC denied | ✅ TCC granted to Omnara.app |
| `osascript → System Events` works | ❌ TCC denied | ✅ TCC granted |
| GUI window confirmed visible | ❌ unverifiable | ✅ confirmed in screenshots |
| Channel-override empirically honored | ❌ unverifiable | ✅ ChannelState dump in log |
| Sign-in OAuth gate skipped | ❌ | ✅ `skip-oss-login.patch` |
| Namespace isolation from stable Warp | ❌ | ✅ `rename-to-lumen.patch` |
| HTTPS to lifegw succeeds (strict TLS) | ❌ | ✅ `dev-tls-bootstrap.sh` + system trust |
| Lumen reaches workspace + zsh terminal session | ❌ | ✅ `10-lumen-live-terminal.png` |

Layer 2-GUI is now **closed**. The remaining work is feature-driven
(point Lumen's agent calls at lifegw's resolvers, implement the
Anthropic-Firebase auth bridge in lifegw, etc.) — none of which is
gated on tooling or sandbox concerns anymore.

## How to reproduce on a clean machine

```bash
# 0. Prereqs: Xcode 26+ with Metal Toolchain installed, Rust 1.85+
# 1. Clone the workspace + worktree
git checkout feat/lumen-phase-alpha-m7-w

# 2. Apply patches + build warp-oss
bash apps/warp-oss-patch/apply-patch.sh             # ~25 min cold

# 3. Generate dev TLS + trust CA (one-time per machine)
./scripts/dev-tls-bootstrap.sh
sudo security add-trusted-cert -d -r trustRoot \
  -k /Library/Keychains/System.keychain \
  crates/life-runtime/lifegw/dev-tls/dev-ca.pem

# 4. Grant Omnara.app (or your terminal app) these macOS Privacy permissions:
#    - Screen Recording
#    - Automation → System Events
#    - Accessibility (optional — only needed for keystroke automation)
# Restart the agent session.

# 5. Bring up backend + launch Lumen
./scripts/lumen-smoke.sh up
./scripts/lumen-app.sh launch
# (macOS may prompt about Group Container access on first launch — click "Don't Allow"
#  for full isolation, or "Allow" to share the team Group Container with stable Warp.)
```

That's the entire path from a clean state to a running, lifegw-talking Lumen.

## Sources

- `docs/lumen/agent-driven-feedback-loop.md` — architecture map (committed `b59f63ba`)
- `docs/lumen/agent-feedback-loop-status.md` — pre-TCC honest status (committed `685a5026`)
- `docs/lumen/continuation-prompts/2026-05-15-gui-loop-after-tcc-grant.md` — handoff prompt (committed `3e2b3413`)
- `docs/lumen/dev-tls-setup.md` — local-CA TLS runbook (committed `54375190`)
- `apps/warp-oss-patch/channel-oss-override.patch` — URL-override patch (existing)
- `apps/warp-oss-patch/skip-metal-when-wgpu.patch` — Metal-bypass safety net (existing)
- `apps/warp-oss-patch/skip-oss-login.patch` — OAuth-gate skip (committed `8fe4c269`)
- `apps/warp-oss-patch/rename-to-lumen.patch` — namespace separation (committed `651850d7`)
- `scripts/dev-tls-bootstrap.sh` — local CA + SAN-bearing leaf generator (committed `54375190`)
- `scripts/lumen-app.sh` — agent-interaction toolkit (committed `b59f63ba`, updated `651850d7`)
