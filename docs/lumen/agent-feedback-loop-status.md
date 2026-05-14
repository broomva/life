# Agent-driven feedback loop — current state

**Date**: 2026-05-14
**Branch**: `feat/lumen-phase-alpha-m7-w`
**Substrate**: shipped (`scripts/lumen-app.sh`, `WarpOss.app` wrapper, Metal-bypass patch)

## TL;DR

The substrate for an agent-driven warp-oss feedback loop is in place. The
binary builds and launches; the headless wire-protocol smoke is green
end-to-end. **What's left is one macOS Privacy & Security toggle on the
agent's parent app** — without it, the OS sandbox blocks the
keystroke / screenshot / window-enumeration calls the loop relies on.

## What works today (agent can do these without any user action)

| Capability | Status | Evidence |
|---|---|---|
| Build the patched warp-oss binary | ✅ | `target/release/warp-oss` (355 MB) |
| Bring up lifegw + lifed | ✅ | `scripts/lumen-smoke.sh up` |
| Launch the binary | ✅ | `scripts/lumen-app.sh launch` → PID + alive |
| Confirm the binary reaches `applicationDidFinishLaunching` | ✅ | `~/Library/Logs/warp-oss.log` |
| Tail logs (lifegw + lifed + warp-oss) | ✅ | `scripts/lumen-app.sh logs tail` |
| Run the headless wire-protocol smoke | ✅ | `cargo run --example lumen_smoke_client` — 5/5 green |
| Validate any change to lifegw/lifed at the wire | ✅ | smoke is the canonical regression check |

## What's blocked on macOS TCC (Privacy & Security)

Three permissions are needed by the agent's parent process to drive
the GUI. The parent app is **Omnara.app** (the harness that runs
this Claude Code session — confirmed via process-ancestry trace).

| Permission | Used for | Symptom when missing |
|---|---|---|
| **Screen Recording** | `screencapture` of any window other than self | `could not create image from display` |
| **Automation → System Events** | `osascript` keystroke / window enumeration / activate | osascript calls hang indefinitely |
| **Accessibility** | Some keystroke / click APIs | (similar — silent failure) |

### To unblock the full agent loop

User performs once:

1. **System Settings → Privacy & Security → Screen Recording**
   - Click `+`, add `/Users/broomva/.omnara/Omnara.app`
   - Toggle ON
2. **System Settings → Privacy & Security → Automation**
   - Expand `Omnara` row
   - Toggle ON for **System Events**
3. **System Settings → Privacy & Security → Accessibility**
   - Click `+`, add `Omnara.app`
   - Toggle ON
4. **Restart the Claude Code / Omnara session** (TCC caches per-process; the new permissions take effect on the next launch)

After that, `scripts/lumen-app.sh loop` becomes the canonical "validate
a code change end-to-end" command — launch the GUI, drive UI, capture
screenshots, diff against baseline.

## Alternative path (no TCC, but engineering work)

Warp-oss has a built-in `run_integration_test(driver: TestDriver)`
entry point gated behind `feature = "integration_tests"` (see
`apps/warp-oss-patch/upstream/app/src/lib.rs:748`). This drives the
app in-process via a Rust `TestDriver` API — no osascript, no
accessibility, no TCC.

Cost: rebuild warp-oss with `--features integration_tests` (we have
this path); write a separate `lumen-integration-test` Rust binary
that uses `TestDriver::new()`, scripts a sequence of test steps
(load UI → focus prompt → type → submit → screenshot), and emits
artifacts to `docs/lumen/validation/`.

This is the "real" path for CI-grade agent validation — TCC works
for ad-hoc human-style poking, TestDriver is what gets wired into a
PR-gating regression suite.

## What the binary actually does on startup (telemetry)

From the most recent run (no user yet, no Anthropic JWT):

```
2026-05-14T22:17:49Z [INFO] Initial open file descriptor limit is 9223372036854775807, ...
2026-05-14T22:17:49Z [INFO] Spawning terminal server process...
2026-05-14T22:17:49Z [INFO] Running terminal server...
2026-05-14T22:17:49Z [INFO] application will finish launching
2026-05-14T22:17:49Z [INFO] Unable to read user from secure storage: SecureStorageError(NotFound)
```

Then the log goes silent but the process keeps running. The
NSApplication is initialized; the keychain lookup correctly reports
no stored user (we have not signed in to Warp via the official
`app.warp.dev` flow); then the next step would normally be either:
- Show the sign-in window (if the binary believes it should)
- Wait for input

We cannot confirm which because the OS won't let us enumerate
windows. That confirmation is exactly what the TCC grant or
TestDriver path unblocks.

## Honest assessment

This is a real product-engineering blocker, not a code bug:

- The wire-protocol gate is **fully closed and green** (Layer 1 + Layer 2-headless)
- The build / launch / log capture substrate is **fully closed**
- The actual GUI interaction loop is gated on a one-time OS-level
  permission grant the agent cannot perform itself
- The TestDriver path is a real engineering project (~half a day of
  Rust work) but produces a CI-grade harness that survives long-term

Both paths are valid. The TCC path is "human-permission-gated, agent
runs forever after"; the TestDriver path is "engineering-gated, no
permission needed". Either way, the substrate that today's commits
deliver is the prerequisite for both.
