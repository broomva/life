---
tags:
  - spec-j
  - phase-1
  - conformance
  - claude-code
  - lifegw
type: runbook
status: ready
area: life-runtime
created: 2026-05-18
spec: docs/superpowers/specs/2026-05-18-spec-j-claude-code-interop.md
plan: docs/superpowers/plans/2026-05-18-spec-j-phase-1-lifegw-edge.md
linear: BRO-1146
---

# Claude Code ↔ lifegw — live conformance smoke runbook (Spec J §J-Sub-G)

**Audience**: operator (the human running the live smoke). The agent-side
prep (test scaffolding, deployment script, STATUS + Spec J doc updates)
is automated and lands in PR for BRO-1146. **This runbook is the
step-by-step the operator executes** once that PR merges.

**Goal**: certify Phase 1 of Spec J end-to-end by running a real
Claude Code CLI session against a Railway-deployed lifegw. Capture
Loom recording + Vigil trace + lago replay + haima ledger as the
evidence surface, then mark BRO-1146 Done.

**Time budget**: ~60 minutes of operator time. The 15+ minute Claude
Code session is the centerpiece; the rest is deploy + capture +
write-up.

**Config-surface honesty note**: lifegw reads configuration from a
single TOML file (default `/etc/lifegw/config.toml` inside the
container, configured via the `LIFEGW_CONFIG` env var the Dockerfile
sets). It does **not** consume `LIFEGW__*`-style env-var overlays. To
enable the dev signer for this smoke, the deploy script edits the
baked `deploy/railway/lifegw-stack/lifegw.toml` in place (flipping
`dev_signer_enabled = false → true`) before `railway up`. A `.bak`
backup is written; the script reminds you to revert it before
committing. See §2.2 below for the full mutation list.

**Pre-merged prerequisite**: the corresponding Phase 1 PRs are on
`main`:

* J-Sub-A → J-Sub-F all merged (`f4963feb` is the most-recent
  Phase 1 merge — J-Sub-E vigil + haima + x402 from BRO-1144 PR
  #1335). The in-process E2E test scaffold ships in *this* PR
  (BRO-1146 prep).

---

## Quick local smoke (zero deploy, zero Railway)

Before the full staging deploy, you can run lifegw locally against a
mock lifed for a ~30 second interactive smoke:

```sh
cargo run -p lifegw --example local_smoke
```

Prints a localhost URL + dev-bearer; `curl` or `claude` against it.
Useful for iterating on the Anthropic Messages route, validating
Vigil span emission, and verifying the wire over a real TCP socket
without burning Railway slots. See
`crates/life-runtime/lifegw/examples/README.md` for the full curl
recipes.

The full Railway staging path (below) is what produces the
Phase 1 conformance evidence.

---

## Section 1 — Prerequisites

Before starting, verify the following are installed and configured on
the operator's workstation:

| Tool | Version | Verify | Install |
|---|---|---|---|
| **Railway CLI** | ≥ 4.0 | `railway --version` | <https://docs.railway.app/develop/cli> |
| **Claude Code CLI** | ≥ 2.0 (post `/model`-discovery) | `claude --version` | <https://docs.anthropic.com/claude/docs/claude-code> |
| **`gh`** | any | `gh auth status` | <https://cli.github.com/> |
| **`cargo` toolchain** | matches workspace MSRV (1.85) | `cargo --version` | rustup |
| **`broomva.tech` Tier-1 JWS** | for a dev user | see below | dev signer (Phase 1) |
| **OTLP-receiving observability platform** | Langfuse OR Grafana Tempo OR Jaeger | endpoint URL ready | tenant of operator's choice |

### 1.1 — Obtain a Tier-1 JWS

Phase 1 ships the dev signer behind `dev_signer_enabled = true`. The
operator's Tier-1 token is:

```text
Bearer dev-token-for-<your-username>
```

For example `Bearer dev-token-for-broomva` resolves a synthetic
`did:life:broomva` at the gateway.

**Production note**: in Phase 2+, the Tier-1 JWS is minted by the
Vercel-hosted broomva.tech `auth.callback` endpoint and verified via
the JWKS at `https://broomva.tech/.well-known/jwks.json`. The dev
signer is OFF in production. For this smoke, dev-mode is the
acceptable Phase 1 surface.

### 1.2 — Railway staging environment

The smoke deploys to a **separate Railway environment** so the live
production lifegw (if any) is untouched. Recommended naming:

```text
service:     lifegw-spec-j
environment: staging
region:      us-west2 (or the region with lowest latency to operator)
```

If the operator has not yet linked the local repo to a Railway
project, run `railway link` and pick the `life-runtime` project (or
create one).

**The target service must already exist** before running the deploy
script — the script's pre-flight calls `railway service link
lifegw-spec-j` and bails if the service is missing. Create it once
via either:

```bash
railway add --service lifegw-spec-j      # CLI
# OR open https://railway.app, navigate to the project, add a new
# empty service named `lifegw-spec-j`.
```

### 1.3 — DNS / public URL

The Loom recording is more legible if the operator's `ANTHROPIC_BASE_URL`
is a stable DNS name rather than a `*.railway.app` hash. Recommended:

```text
https://lifegw-spec-j.broomva.dev
```

Configure via Railway's "Public networking" panel or by adding a
CNAME from `lifegw-spec-j.broomva.dev` → `<service>.railway.app` in
the broomva.dev DNS zone.

---

## Section 2 — Deploy lifegw to staging

The wrapped commands live in
`scripts/deploy_lifegw_staging.sh`. The script is **idempotent** —
re-running it produces the same end state. The operator can either
run the script end-to-end OR paste the commands one at a time. Both
paths are documented below.

### 2.1 — One-shot deploy

```bash
bash scripts/deploy_lifegw_staging.sh
```

The script will:

1. Validate the Railway CLI is logged in (`railway whoami`).
2. Confirm the target service `lifegw-spec-j` exists in the linked
   project (via `railway service link`), and bail with a clear
   creation instruction if it doesn't.
3. Check the active branch is `main` (warn if not).
4. Confirm the deployment with the operator (no auto-deploy without an
   explicit `--yes` flag).
5. **Patch the baked `deploy/railway/lifegw-stack/lifegw.toml`** in
   place to set `dev_signer_enabled = true` (see §2.2). A `.bak`
   backup is written; the operator must revert before committing.
6. Set the small set of env vars Railway *does* consume (see §2.2).
7. `railway up --service lifegw-spec-j --environment staging`.
8. Wait for the new deployment to become healthy (`/healthz` returns
   200). The script **bails on health-probe failure**; a missing
   `/healthz` after 5 minutes is treated as a hard failure, not a
   warning, because the smoke cannot succeed against a half-up
   deploy.
9. Print the public URL + the `ANTHROPIC_BASE_URL` +
   `ANTHROPIC_AUTH_TOKEN` the operator should set in Section 3, and
   remind the operator to revert the TOML edit before committing.

### 2.2 — Configuration: TOML edits + Railway env vars

lifegw reads only the TOML pointed at by `LIFEGW_CONFIG`. The
Dockerfile bakes `LIFEGW_CONFIG=/etc/lifegw/config.toml` and copies
`deploy/railway/lifegw-stack/lifegw.toml` into the image at build
time. To enable the dev signer for this smoke, the deploy script
**edits that file in place** before triggering `railway up`. The
table below documents the mutation; the deploy script does it
automatically. The operator who runs the commands manually must
apply the same edit.

#### 2.2a — TOML mutations (committed-but-pre-deploy edit)

| File | Field | Before | After | Why |
|---|---|---|---|---|
| `deploy/railway/lifegw-stack/lifegw.toml` | `[auth] dev_signer_enabled` | `false` | `true` | accepts `Bearer dev-token-for-{user}` for the smoke |

The deploy script writes a `.bak` backup next to the file before
patching. After teardown, restore the original:

```bash
mv deploy/railway/lifegw-stack/lifegw.toml.bak \
   deploy/railway/lifegw-stack/lifegw.toml
```

**Do not commit the patched TOML.** The repo's main-branch posture is
production (`dev_signer_enabled = false`); the smoke is an ephemeral
opt-in.

The `[billing] enforce` flag does NOT exist in the baked TOML
(`StubHaimaClient` is wired at the code level in BRO-1144; ledger is
empty across the smoke regardless of the flag). The Phase 1 default
is "billing not enforced" by construction. The runbook §6.1
known-limitations log carries this honestly.

#### 2.2b — Railway-level env vars (set on the service)

These vars are read by **Railway's build system** (not by lifegw) or
by **the entrypoint/vigil layer** (not lifegw's config loader):

| Variable | Value | Consumed by | Why |
|---|---|---|---|
| `RAILWAY_DOCKERFILE_PATH` | `deploy/railway/lifegw-stack/Dockerfile` | Railway build picker | without this, Railway nixpacks-autodetects the monorepo and fails to use the multi-process Dockerfile |
| `LIFEGW_OTLP_ENDPOINT` | `<langfuse-or-tempo-endpoint>` | `life_vigil::init_telemetry` | so Vigil traces reach the operator's observability platform |
| `OTEL_SERVICE_NAME` | `lifegw-spec-j` | OpenTelemetry SDK | distinct service-name tagging vs production |
| `LIFED_ALLOW_MOCK_FALLBACK` | `true` | lifed bootstrap | mock substrate fallback so the smoke runs against the lifegw-stack image without real lago/anima/haima daemons (this is the Dockerfile default; setting it on the service makes it operator-flippable without redeploying) |

The deploy script sets these via `railway variables --set`. Manual
operators run the same commands one at a time.

**Important**: the **lifed substrate** runs *inside the same
container* as lifegw via the multi-process `lifegw-stack` image (see
`deploy/railway/lifegw-stack/Dockerfile` + `entrypoint.sh`). lifed
binds the UDS at `/run/life/life.sock` that lifegw's TOML points
to. This is the Topology A path documented at
`deploy/railway/lifegw-stack/README.md`.

* **Option A (in use here)** — single-container `lifegw-stack` image
  with lifed + lifegw + caddy fanned out via tini. `lifed` runs with
  `LIFED_ALLOW_MOCK_FALLBACK=true`, so arcand/lagod/haimad/animad/soma
  are mocked. The full wire path through Caddy → lifegw → lifed is
  real; substrate behavior is mocked.
* **Option B (production-faithful, not used for this smoke)** — lifed +
  arcand + lagod + haimad as separate Railway services / sidecars
  over Railway volumes for the shared UDS dir. This matches Topology
  B and is the surface post-Phase-1 work exercises.

Phase 1 of Spec J is wire-shape only. **Option A is sufficient for
the BRO-1146 smoke** and is what the `lifegw-stack` Dockerfile +
`scripts/deploy_lifegw_staging.sh` deploy. Document Option A in
Section 7's write-up.

### 2.3 — Smoke-test the deploy

After `railway up` returns, run a curl probe to confirm the gateway
is up:

```bash
# Health probe — no auth required.
curl -fsS "https://lifegw-spec-j.broomva.dev/healthz"
# Expected: 200 OK + body "ok"

# Models endpoint — no auth required.
curl -fsS "https://lifegw-spec-j.broomva.dev/v1/models" | jq '.data[].id'
# Expected: at least 5 model IDs, including claude-opus-4-20250514,
# claude-sonnet-4-20250514, claude-haiku-4-20250514, etc.

# Simple chat — auth required.
curl -fsS "https://lifegw-spec-j.broomva.dev/v1/messages" \
  -H "authorization: Bearer dev-token-for-broomva" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{"model":"claude-sonnet-4-20250514","messages":[{"role":"user","content":"ping"}],"max_tokens":50,"stream":true}' \
  | head -20
# Expected: text/event-stream body with event: message_start, then
# content_block_delta frames, terminating in event: message_stop.
```

If any of the three probes fails, **fix before proceeding**.
Pre-flight failures during the live session are unrecoverable for
the smoke purposes.

---

## Section 3 — Configure Claude Code

Claude Code reads `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` from
the process environment. Set them in the shell the operator will run
`claude` from:

```bash
export ANTHROPIC_BASE_URL="https://lifegw-spec-j.broomva.dev"
export ANTHROPIC_AUTH_TOKEN="dev-token-for-broomva"
# Optional but recommended — give Claude Code's auto-compaction loop
# the same 190K-token budget it uses against api.anthropic.com.
export CLAUDE_CODE_AUTO_COMPACT_WINDOW=190000
# Optional — enable Claude Code's gateway-aware /model picker.
export CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1
```

Then launch Claude Code in the chosen working directory:

```bash
cd ~/broomva/core/life  # or wherever the operator wants the agent to work
claude
```

The Claude Code TUI should come up. The status line should not show
any auth-error banner. If it does, double-check the token format
(`Bearer ` prefix is added by Claude Code; `ANTHROPIC_AUTH_TOKEN` is
just the token body).

---

## Section 4 — Smoke test session

The session must exercise the following invariants. **Total session
duration ≥ 15 minutes** of active work — not 15 minutes of `claude`
idle in the foreground.

### 4.1 — Three tool calls (minimum)

The first three tool calls must be:

1. **File read** — ask Claude Code to read a real file in the working
   directory, e.g. `read crates/life-runtime/lifegw/src/services/anthropic_messages.rs` or
   `show me the first 50 lines of CLAUDE.md`.
2. **File edit** — ask Claude Code to make a small no-op edit, e.g.
   `add a trailing newline to docs/conformance/_test.txt` (operator
   pre-creates the file). Verify the edit lands on disk.
3. **Bash command** — ask Claude Code to run a shell command, e.g.
   `run cargo --version` or `count the rust files under crates/`.

After each call, verify in the Claude Code UI that the tool fired
without an error and produced sensible output. The codec is on the
hot path for each of these; a malformed `tool_use` block will surface
as an immediate parse error in Claude Code.

### 4.2 — Mid-stream connection drop

While Claude Code is mid-response (the agent should be visibly
streaming output), drop the local network for 5-10 seconds. The
easiest way:

```bash
# In a separate terminal:
sudo ifconfig en0 down && sleep 8 && sudo ifconfig en0 up
# Adjust `en0` to the operator's primary interface (could be wlan0 on Linux).
```

Claude Code should display a transient reconnect indicator, then
resume cleanly. If it surfaces a hard error, capture the screen state
+ Vigil span ID and note it in Section 7's known-limitations log.

### 4.3 — `/model` switch via picker

In the Claude Code TUI, type `/model` to bring up the model picker.
The picker should show the five Anthropic-pinned ids from the
gateway's `/v1/models` response. Pick a different model than the
current one (e.g. switch from sonnet-4 → haiku-4, or vice versa).
Send one more message and confirm it routes correctly.

### 4.4 — Continued work (filler)

Use the remaining session time for ordinary coding work — answer
questions about the repo, ask Claude Code to grep for patterns, run
a test, sketch a small refactor, etc. The goal is to accumulate
≥ 15 minutes of real session time so the Loom recording shows the
gateway sustaining real load, not just three probe calls.

---

## Section 5 — Evidence capture

Each artifact below lands in a specific path in the repo. The PR
that closes BRO-1146 should reference all of them.

### 5.1 — Loom (or equivalent) screen recording

* Format: Loom is preferred (shareable URL, no file in repo). If
  Loom isn't available, OBS → mp4 also works; upload to broomva.dev
  and link.
* Length: 15-20 minutes (the full session).
* Content: full Claude Code TUI + the operator's terminal showing
  `ANTHROPIC_BASE_URL` and the network drop.
* Path: link in
  `docs/conformance/2026-05-18-claude-code-smoke-results.md` (Section
  7 below).

### 5.2 — Vigil trace screenshot

* Source: the OTLP-receiving platform from Section 1's prereqs
  (Langfuse / Tempo / Jaeger).
* Content: the trace timeline for one of the tool-call requests
  showing `life.anthropic.messages` root → child spans
  (`life.anthropic.auth_verify`, `life.anthropic.sid_synthesis`,
  `life.anthropic.haima_check`, `life.anthropic.codec_encode`).
* Format: PNG screenshot.
* Path: `docs/conformance/evidence/2026-05-18-vigil-trace-tool-call.png`.

### 5.3 — `lago replay --tree` output

Pick one of the conversation sids from the Vigil span attributes
(`life.session.id`). On the staging deploy, SSH into the running
container and execute `lago replay --tree`:

```bash
# `railway ssh` (with -- to pass a command) opens a shell INSIDE
# the deployed container. NOTE: `railway shell` is a different
# command — it opens a LOCAL subshell with Railway env vars
# exported, which is not what we want here.
railway ssh --service lifegw-spec-j --environment staging -- \
  lago replay --tree <synthesized_sid>
```

Capture stdout to:

```text
docs/conformance/evidence/2026-05-18-lago-replay-tree.txt
```

**Plan-tier note**: `railway ssh` into a running deployment may be
restricted on Free/Hobby Railway plans. If SSH is unavailable on the
operator's plan, capture the same output via the Railway Dashboard's
in-browser shell, OR skip this evidence and record the gap in
Section 7's known-limitations log (lago replay is also recorded
asynchronously by the lago substrate — a follow-up PR can fetch the
trace from the lagod journal).

**Substrate-mode note**: Option A (the `lifegw-stack` Dockerfile in
use here) runs `lifed` with `LIFED_ALLOW_MOCK_FALLBACK=true`, which
mocks `lagod`. Real `lago replay --tree` requires Option B
(production-faithful deploy with a real lagod). For this Phase 1
smoke, recording "lago replay unavailable due to Option A mock
substrate" in §7's known-limitations log is the expected outcome;
the live lago-replay evidence is the surface BRO-1147+ exercises.

### 5.4 — `haima ledger show` output

Same SSH access pattern (see §5.3's plan-tier + Dashboard fallback
guidance):

```bash
railway ssh --service lifegw-spec-j --environment staging -- \
  haima ledger show did:life:broomva
```

Capture to:

```text
docs/conformance/evidence/2026-05-18-haima-ledger-show.txt
```

**Note**: with the Phase 1 `StubHaimaClient` wired in BRO-1144 (no
billing-enforce flag exists in the baked TOML — billing is unwired
at the code level), the haima ledger will be empty. **This is
expected.** The Phase 1 surface that `BRO-1144` PR #1335 shipped
records usage on the Vigil span but does not commit ledger entries
because the live haima client is post-Phase-1. The ledger-empty
output is the correct evidence for Phase 1 acceptance; live ledger
entries are the surface BRO-1147+ owns.

### 5.5 — Test scaffold output

For completeness, attach the output of the in-process E2E test that
lands in this PR:

```bash
cd ~/broomva/core/life
cargo test -p lifegw --test spec_j_e2e_smoke 2>&1 | tee \
  docs/conformance/evidence/2026-05-18-in-process-e2e-output.txt
```

Expected: 5 tests pass, 0 failed.

---

## Section 6 — Acceptance criteria

The smoke is **green** when **all** of the following hold:

1. **Wire-shape compatibility** — Claude Code's TUI completes ≥ 3
   tool calls without any client-side parse error and without
   surfacing the "Connection reset by peer" or "Bad gateway"
   transient that points to a codec-shape bug at the gateway.
2. **Mid-stream drop recovery** — Claude Code reconnects cleanly
   after the Section 4.2 network drop and continues the
   conversation without losing context.
3. **Model picker works** — `/model` lists ≥ 5 ids; switching
   models produces a clean response from the newly-selected model.
4. **Vigil trace exists** — the screenshot in 5.2 shows the full
   span hierarchy.
5. **Session duration** — Loom recording shows ≥ 15 minutes of
   active interaction.
6. **In-process E2E green** — `cargo test -p lifegw --test
   spec_j_e2e_smoke` passes 5/5 on the current branch.

The smoke is **failed** when any of the following hold:

* Claude Code's TUI shows a persistent error banner that doesn't
  clear after a fresh request.
* The `/v1/models` endpoint returns a body Claude Code can't parse.
* The mid-stream drop test causes a TUI lockup that requires
  killing the `claude` process.
* The `tool_use` round-trip is broken end-to-end (the agent
  emits a tool call, the client runs the tool, the response goes
  back, and the agent doesn't pick up the tool_result).

### 6.1 — Known limitations carried from BRO-1144 PR #1335

These are **expected to surface** during the smoke and **do not
count as failures**:

* **`traceparent` not propagated to lifed** — Vigil trace tree shows
  `life.anthropic.messages` root + child spans within the lifegw
  process; the lifed-side spans appear as a separate trace. The
  W3C tracecontext propagation gap is tracked under a Phase 2
  follow-up.
* **`StubHaimaClient` returns `Ok(_)` unconditionally** — haima
  ledger remains empty across the smoke. This is the documented
  Phase 1 default (billing is unwired at the code level; the live
  haima client is post-Phase-1 work tracked under BRO-1147+).
* **`/v1/messages/count_tokens` is approximate** — the edge
  estimator uses Vigil's `chars/4` heuristic. ±5% accuracy vs
  Anthropic's reference. Acceptable for Phase 1.

---

## Section 7 — Filing the results

After the smoke is complete (green or failed):

### 7.1 — Write the results doc

Create `docs/conformance/2026-05-18-claude-code-smoke-results.md`
with the following template:

```markdown
---
spec: docs/superpowers/specs/2026-05-18-spec-j-claude-code-interop.md
runbook: docs/conformance/2026-05-18-claude-code-smoke-runbook.md
linear: BRO-1146
result: <green | failed | conditional>
operator: <github-handle>
date: 2026-05-XX
deploy_option: <A | B>  # see Section 2.2
---

# Spec J Phase 1 — Claude Code smoke results

## Summary

(1-2 paragraphs of what happened.)

## Evidence

- Loom recording: <URL>
- Vigil trace: docs/conformance/evidence/2026-05-18-vigil-trace-tool-call.png
- lago replay: docs/conformance/evidence/2026-05-18-lago-replay-tree.txt
- haima ledger: docs/conformance/evidence/2026-05-18-haima-ledger-show.txt
- in-process E2E: docs/conformance/evidence/2026-05-18-in-process-e2e-output.txt

## Acceptance walkthrough

1. Wire-shape compatibility: <PASS | FAIL — details>
2. Mid-stream drop recovery: <PASS | FAIL — details>
3. Model picker: <PASS | FAIL — details>
4. Vigil trace: <PASS | FAIL — details>
5. Session duration: <duration> minutes
6. In-process E2E: <5/5 pass>

## Known limitations observed

(List any of the BRO-1144 carry-over gaps actually surfaced + any
new gaps discovered.)

## Follow-ups filed

(List Linear tickets filed for any post-smoke work.)
```

### 7.2 — Update Linear BRO-1146

Move BRO-1146 to **Done** when the smoke is green. Attach the
results doc URL + the Loom URL in the ticket body. If the smoke is
**conditional** (green except for known-limitations), open a
follow-up ticket explicitly naming the gap and link it from
BRO-1146 before closing.

### 7.3 — Update STATUS.md

Append a Spec J Phase 1 shipped entry to `docs/STATUS.md` matching
the pattern of M5/M7/Spec D entries — date, PR links, test counts
(in-process + live smoke), evidence-doc paths.

### 7.4 — Update Spec J header

Change the `Status:` line at the top of the Spec J spec from
"Draft" or "Phase 1 (5/6) shipped 2026-05-18; E2E smoke runbook
ready for operator execution (BRO-1146)" → "Phase 1 SHIPPED
2026-05-XX (results: docs/conformance/2026-05-18-claude-code-smoke-results.md)".

### 7.5 — Tear down staging

After results are filed, tear down the staging deploy unless it's
useful as a development sandbox:

```bash
# Removes the most recent deployment but keeps the service shell so
# re-running scripts/deploy_lifegw_staging.sh is a one-command
# re-deploy. This is the CLI surface Railway 4.x supports.
railway down --service lifegw-spec-j --environment staging --yes
```

To **fully delete the service** (so it stops appearing in the
project), use the Railway Dashboard — the CLI does not expose a
`service delete` subcommand in 4.x:

```text
1. open https://railway.app
2. navigate to the project → service "lifegw-spec-j"
3. Settings → Delete service
```

After tear-down, also revert the TOML mutation the deploy script
made:

```bash
mv deploy/railway/lifegw-stack/lifegw.toml.bak \
   deploy/railway/lifegw-stack/lifegw.toml
```

This keeps the Railway bill clean and the repo at its production
posture. The smoke evidence is permanent in-repo; the running service
is not.

---

## Appendix A — Troubleshooting

### A.1 — `/v1/messages` returns 502 immediately

* lifed is not running on the upstream UDS, OR the lifegw config's
  `[upstream] lifed_uds_path` doesn't match the lifed bind path.
* Fix: verify `railway logs --service lifegw-spec-j` shows
  `dial_upstream` succeeding. If not, SSH into the container and
  inspect both sides:

  ```bash
  railway ssh --service lifegw-spec-j --environment staging -- \
    sh -c 'grep -E "lifed_uds_path|lago.sock|life.sock" /etc/lifegw/config.toml /etc/lifed/config.toml; ls -la /run/life/'
  ```

  Expect both configs to reference `/run/life/life.sock` and that
  socket to exist with mode `srw-rw----`.

### A.2 — Claude Code shows "authentication_error"

* The dev signer is not enabled, OR the token format is wrong.
* Fix (TOML side — the only one lifegw reads): SSH into the
  container and check the baked config:

  ```bash
  railway ssh --service lifegw-spec-j --environment staging -- \
    grep dev_signer_enabled /etc/lifegw/config.toml
  # Expected: dev_signer_enabled = true
  ```

  If it shows `false`, the deploy script's TOML edit didn't take
  (perhaps the operator deployed without the script, or the
  pattern-match failed). Re-run `bash scripts/deploy_lifegw_staging.sh`
  to re-apply the patch + re-deploy.
* Fix (token side): the token body (after `Bearer `) should be
  `dev-token-for-<username>`. Claude Code adds the `Bearer ` prefix
  automatically; `ANTHROPIC_AUTH_TOKEN` carries just the body.

### A.3 — Vigil traces don't show up

* OTLP endpoint env var is missing or pointing to the wrong URL.
* Fix: `railway logs --service lifegw-spec-j | grep otlp` should
  show `OTLP exporter initialized — endpoint: <url>`. If the line
  shows `OTLP endpoint not configured — falling back to structured
  logging`, the env var didn't take.

### A.4 — `/model` picker shows only one model

* Claude Code's `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` isn't
  set; it defaulted to Anthropic's hardcoded list.
* Fix: export the env var before launching `claude`. Verify
  via `curl https://lifegw-spec-j.broomva.dev/v1/models` returns
  ≥ 5 ids.

### A.5 — In-process E2E test fails locally

* The workspace's MSRV is 1.85; older toolchains produce edition
  2024 compile errors.
* Fix: `rustup update stable` then re-run.

---

## Appendix B — Future runbook iterations

This runbook is **Phase 1 specific**. Phase 2 surfaces (J-Sub-H
through J-Sub-J — AnthropicArcan promotion, Praxis-side tool
execution, life-claude launcher) will replace several manual steps
here:

* The Phase 1 "billing not enforced" default (currently a code-level
  posture via `StubHaimaClient`) goes to enforcement after BRO-1147+
  wires the live haima client.
* The TOML `dev_signer_enabled = true` flag (the smoke's manual
  pre-deploy edit, see §2.2a) stays `false` permanently once the
  Tier-1 JWS mint is hosted at broomva.tech `auth.callback` and the
  smoke flow runs without dev shortcuts.
* The `apps/life-claude` launcher replaces the manual
  `ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` env-var dance — the
  operator runs `life claude` and the launcher handles the env
  setup + x402 topup interception.

Re-publish this runbook (`2026-XX-XX-claude-code-smoke-runbook-v2.md`)
when those surfaces land.

