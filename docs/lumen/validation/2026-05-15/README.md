# Lumen Phase α — Layer 2-GUI empirical evidence (2026-05-15)

**Branch**: `feat/lumen-phase-alpha-m7-w`
**Predecessor**: [`docs/lumen/validation/2026-05-12/`](../2026-05-12/) (Layer 1 + Layer 2-headless, 5/5 green)
**Operator**: agent-driven via `scripts/lumen-app.sh` after macOS TCC grants to Omnara.app

## TL;DR

For the first time, an agent has empirically observed the patched
warp-oss GUI talking to (almost) lifegw. Three of the four
remaining Layer 2-GUI gates are now closed:

| Gate | Status |
|---|---|
| Binary launches with full NSApplication lifecycle | ✅ |
| Channel-override patch honored (`WARP_SERVER_ROOT_URL` flows through) | ✅ |
| GUI window renders, onboarding flow is interactive | ✅ |
| HTTPS connection from warp-oss to lifegw succeeds | ❌ **TLS trust gate** |
| Sign-in step completes against a local IdP | ❌ **OAuth gate (browser flow)** |

The two remaining gates are real **product** gaps (not tooling gaps)
and are addressable with two small, well-scoped patches.

## Evidence files

| File | What it captures |
|---|---|
| `01-warp-oss-launched.png` | "Customize your Warp" — onboarding slide 3 (Tools panel + tab styling) |
| `02-onboarding-mid-flow.png` | "Sign in on your browser to continue" — the sign-in gate |
| `03-onboarding-current.png` | Same screen ~2 s later (user did not advance — confirming gate) |
| `lifegw-during-gui.log` | lifegw log slice (NOTE: bootstrap-only — no incoming HTTPS requests) |
| `warp-oss-during-gui.log` | full warp-oss log including the 6 TLS-cert failures and 15 onboarding click events |

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

### ❌ 4. TLS verification rejects the dev-tls cert (gate #1)

Every HTTPS attempt to lifegw fails with `-67843` (host name mismatch
or untrusted root):

```
[ERROR] failed to verify TLS certificate: invalid peer certificate:
        Other(OtherError("\"localhost\" certificate is not trusted: -67843"))
[WARN]  Failed to fetch free-tier LLMs from server: error sending request
        for url (https://127.0.0.1:8443/graphql/v2?op=FreeAvailableModels):
        invalid peer certificate
```

Six such failures in the GUI run. lifegw's bootstrap-only log
confirms **zero** HTTPS requests reached it — warp-oss's strict TLS
gate blocks every connection before any handshake.

The headless `lumen_smoke_client` doesn't hit this because it calls
`reqwest::Client::builder().danger_accept_invalid_certs(true)`. The
real warp-oss client has no such bypass.

### ❌ 5. Onboarding sign-in is browser-OAuth-based (gate #2)

The current state (per screenshots 02 + 03):

> **Sign in on your browser to continue**
>
> If your browser hasn't opened, copy the URL …

warp-oss expects to delegate auth to `app.warp.dev`'s OAuth flow.
With our channel-override redirecting to `127.0.0.1:8443`, the
auth URL points to lifegw — but TLS blocks that, AND lifegw doesn't
implement Warp's OAuth flow regardless.

## Next-step patches (both small, both well-scoped)

### Patch A — TLS trust escape hatch for Channel::Oss

Two options; **A.1** is the right long-term shape:

**A.1 — env-flag bypass via channel-oss-override** (~10 LOC):
add `WARP_ACCEPT_INVALID_CERTS=1` honoring in the OSS channel's
HTTP-client construction. Reversible per-launch, defaults closed,
matches the existing `WARP_SERVER_ROOT_URL` override shape.

**A.2 — System-keychain trust** (one-time user action, no code):
```bash
sudo security add-trusted-cert -d -r trustRoot \
  -k /Library/Keychains/System.keychain \
  crates/life-runtime/lifegw/dev-tls/cert.pem
```
Simpler today but doesn't scale to other developers.

### Patch B — Skip the sign-in onboarding step in Channel::Oss

The `onboarding::slides::sign_in_slide` (or whatever the precise
module name is — find via `grep` next) needs an `if
ChannelState::channel() == Channel::Oss { skip }` branch, OR the
`OnboardingFlow` needs to advance past the sign-in slide when
running on the OSS channel. This is what the user surfaced
explicitly:

> "Starting the onboarding, there is a step to login to warp servers.
> We need to disable [it]."

Same pattern as `channel-oss-override.patch` — minimal, OSS-channel-
scoped, additive.

## Validation cadence (going forward)

Each iteration of Patch A / Patch B:

```bash
cd apps/warp-oss-patch/upstream
cargo build --release --bin warp-oss            # ~30 s incremental
cd -
./scripts/lumen-app.sh down
./scripts/lumen-app.sh launch
sleep 6
/usr/sbin/screencapture -x docs/lumen/validation/$(date -u +%F)/N-after-patch.png
./scripts/lumen-app.sh logs grep "graphql/v2"   # confirm lifegw is being called
./scripts/lumen-app.sh logs grep "verify TLS"   # confirm count drops to 0
```

The smoke client stays the canonical wire-level regression check.

## What was unblocked today vs prior session

| State | 2026-05-14 | 2026-05-15 |
|---|---|---|
| Metal Toolchain | Apple CDN failed | **Installed** (Apple CDN succeeded on retry) |
| warp-oss binary built | ❌ | ✅ (`.target/release/warp-oss`, 355 MB) |
| `.app` bundle wrapper | ❌ | ✅ (`/tmp/lumen-app/WarpOss.app`) |
| `screencapture` works for the agent | ❌ TCC denied | ✅ TCC granted |
| `osascript → System Events` count | ❌ TCC denied | ✅ TCC granted |
| osascript → window enumerate via Accessibility | ❌ | ❌ (Accessibility not granted) |
| GUI window confirmed visible | ❌ unverifiable | ✅ confirmed in screenshots |
| Channel-override empirically honored | ❌ unverifiable | ✅ confirmed in log |
| HTTPS to lifegw works | ❓ | ❌ **TLS gate** |
| Sign-in completes | ❓ | ❌ **OAuth gate** |

Layer 2-GUI is now **partially closed** — three of five gates green,
two next-step patches identified with concrete shape and scope.

## Sources

- `docs/lumen/agent-driven-feedback-loop.md` — architecture map (committed `b59f63ba`)
- `docs/lumen/agent-feedback-loop-status.md` — pre-TCC honest status (committed `685a5026`)
- `docs/lumen/continuation-prompts/2026-05-15-gui-loop-after-tcc-grant.md` — handoff prompt (committed `3e2b3413`)
- `apps/warp-oss-patch/channel-oss-override.patch` — the URL-override patch (proven working today)
