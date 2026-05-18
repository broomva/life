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

**Pre-merged prerequisite**: the corresponding Phase 1 PRs are on
`main`:

* J-Sub-A → J-Sub-F all merged (`f4963feb` is the most-recent
  Phase 1 merge — J-Sub-E vigil + haima + x402 from BRO-1144 PR
  #1335). The in-process E2E test scaffold ships in *this* PR
  (BRO-1146 prep).

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
2. Check the active branch is `main` (warn if not).
3. Confirm the deployment with the operator (no auto-deploy without an
   explicit `--yes` flag).
4. Set the required env vars (see 2.2 below).
5. `railway up --service lifegw-spec-j --environment staging`.
6. Wait for the new deployment to become healthy (`/healthz` returns
   200).
7. Print the public URL + the `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN`
   the operator should set in Section 3.

### 2.2 — Required environment variables

The script sets these via `railway variables set`. If the operator
runs the deploy manually, the variables must be set on the Railway
service:

| Variable | Value | Why |
|---|---|---|
| `LIFEGW__PUBLIC__BIND_ADDR` | `0.0.0.0:8443` | Railway exposes 8443 to the public-facing load balancer |
| `LIFEGW__PUBLIC__TLS_CERT_PATH` | `/etc/lifegw/tls/cert.pem` | mounted from Railway secret; or use Railway's built-in TLS termination |
| `LIFEGW__PUBLIC__TLS_KEY_PATH` | `/etc/lifegw/tls/key.pem` | as above |
| `LIFEGW__AUTH__DEV_SIGNER_ENABLED` | `true` | accepts `Bearer dev-token-for-{user}` for the smoke |
| `LIFEGW__BILLING__ENFORCE` | `false` | Phase 1 default per BRO-1144 — `StubHaimaClient` is wired; the live haima client lands post-Phase 1 |
| `LIFEGW__OBSERVABILITY__OTLP_ENDPOINT` | `<langfuse-or-tempo-endpoint>` | so the Vigil traces show up in the operator's chosen observability platform |
| `OTEL_SERVICE_NAME` | `lifegw-spec-j` | so spans tag distinctly from production |
| `LIFEGW__UPSTREAM__UDS_PATH` | `/run/life/lifed.sock` | the lifed UDS the gateway proxies to |

**Important**: the **lifed substrate** also needs to be running on the
same Railway service (or a sibling service over UDS-via-volume). If
no lifed substrate is wired, every `/v1/messages` call returns 502
because the upstream tonic channel is broken. For this smoke, either:

* **Option A** (simpler) — deploy a single-binary `arcan serve` image
  that bundles a minimal lifed-equivalent for development. This is
  the Topology A path. The mock `cfg.billing.enforce = false` keeps
  the substrate substrate-light.
* **Option B** (production-faithful) — deploy lifed + arcand + lagod +
  haimad as separate Railway services or sidecars connected via
  Railway volumes. This matches Topology B.

Phase 1 of Spec J is wire-shape only. **Option A is sufficient for
the BRO-1146 smoke**; Option B is the surface the post-Phase-1 work
exercises. Document which option the operator chose in Section 7's
write-up.

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
(`life.session.id`). On the staging deploy, SSH or `railway shell`
into the service and run:

```bash
lago replay --tree <synthesized_sid>
```

Capture stdout to:

```text
docs/conformance/evidence/2026-05-18-lago-replay-tree.txt
```

**Note**: this requires Option B (production-faithful deploy with
real lagod). For Option A (single-binary arcan), `lago replay` is
not available; record this gap in Section 7's known-limitations log.

### 5.4 — `haima ledger show` output

Same shell access pattern:

```bash
haima ledger show did:life:broomva
```

Capture to:

```text
docs/conformance/evidence/2026-05-18-haima-ledger-show.txt
```

**Note**: with `LIFEGW__BILLING__ENFORCE=false` and the Phase 1
`StubHaimaClient`, the haima ledger will be empty. **This is
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
  Phase 1 default (`LIFEGW__BILLING__ENFORCE=false`).
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
railway environment delete staging --service lifegw-spec-j
# OR
railway service delete lifegw-spec-j
```

This keeps the Railway bill clean. The smoke evidence is permanent
in-repo; the running service is not.

---

## Appendix A — Troubleshooting

### A.1 — `/v1/messages` returns 502 immediately

* lifed is not running on the upstream UDS.
* Fix: verify `railway logs --service lifegw-spec-j` shows
  `dial_upstream` succeeding. If not, check `LIFEGW__UPSTREAM__UDS_PATH`
  matches the lifed bind path.

### A.2 — Claude Code shows "authentication_error"

* The dev signer is not enabled, OR the token format is wrong.
* Fix: `railway variables --service lifegw-spec-j | grep DEV_SIGNER`
  should show `LIFEGW__AUTH__DEV_SIGNER_ENABLED=true`. The token
  body (after `Bearer `) should be `dev-token-for-<username>`.

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

* The `LIFEGW__BILLING__ENFORCE=false` Phase 1 default goes to
  `true` after BRO-1147+ wires the live haima client.
* The "dev signer enabled" env var goes off after the Tier-1 JWS
  mint is hosted at broomva.tech `auth.callback`.
* The `apps/life-claude` launcher replaces the manual
  `ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` env-var dance — the
  operator runs `life claude` and the launcher handles the env
  setup + x402 topup interception.

Re-publish this runbook (`2026-XX-XX-claude-code-smoke-runbook-v2.md`)
when those surfaces land.

