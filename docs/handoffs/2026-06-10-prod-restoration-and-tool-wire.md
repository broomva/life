# Prod restoration + chat tool wire — Stage 3

**TL;DR.** Dogfooding the deployed chat surface with live log tracing
exposed that prod had silently decayed: **all four substrate Railway
deploys had been failing for 3 weeks** (workspace-manifest break from
#1415), prod lifed ran fully mocked, prod arcand predated the #1686
tool wire, and the chat UI advertised 20 tools the model never
received. This session repaired the deploy pipeline, restored all
services to current main, closed four architecture gaps via parallel
workflow streams (implement → adversarial verify → fix), and staged
real-arcan activation in lifegw-stack. **Six PRs merged** (#1691,
#1692, #1694, #1695, #1696, #1697), two ops fixes applied live
(`LAGO_URL` :3001→:8080 on arcan; `ARCAN_PROVIDER=openai` staged on
lifegw-stack). **FIRST ACTION:** verify the lifegw-stack deploy at
`d09257b0` booted with `substrates: arcan=real lago=mock haima=mock
anima=mock`, then dogfood chat tools end-to-end (the §Pickup receipt).

## State of the world (P15 snapshot 2026-06-10 ~20:30 -05)

- **main** @ `d09257b0`. Merged this session, in order: #1691
  (`4c74edbc` Dockerfile contexts), #1692 (`f7d18172` agents in arcan
  image), #1694 (`a2ca1a7e` attestation boot wiring), #1696
  (`0caa70bd` agent test --live, closes BRO-1008), #1697 (`6b5c4c74`
  client tool definitions wire), #1695 (`d09257b0` per-substrate
  rollout + arcand in lifegw-stack).
- **OPEN PR (1):** #1693 (chronosd heartbeat-test flake hardening) —
  blocked twice by a *runner-death* pattern in Test (Linux)
  (`cargo test --workspace` step ends with null conclusion = OOM/disk
  kill, not a test failure). Rebased onto `d09257b0`; if it reds a
  third time, investigate the runner cache
  (`v0-rust-test-linux-…f523904f…`) or split the workspace test lane.
- **Railway prod (Life project)**: all 6 services SUCCESS on current
  main. arcan: agents registry loads 9 agents (boot-log verified),
  `LAGO_URL` fixed to `:8080`. lifegw-stack: rebuild at `d09257b0`
  was QUEUED at handoff time — `ARCAN_PROVIDER=openai` already set, so
  this deploy should start real arcand on `/run/life/arcan.sock`.
- **Vercel**: broomva.tech unchanged (chat client repo is separate).
  Known client-side items: `REDIS_URL` unreachable (resumable streams
  off), one custody mint 503 observed (lifegw direct probe returns
  200 — Vercel-side/auth/transient, not life-repo).
- **Conductor workspace** `nicosia`, branch
  `docs/stage3-prod-restoration` (this doc + CLAUDE.md update).
  Workflow worktrees under
  `~/broomva/core/life/.claude/worktrees/wf_37b210dc-*` can be pruned.

## What this session delivered (don't redo it)

| PR | Merge | What it gave |
|----|-------|--------------|
| #1691 | `4c74edbc` | `docker/{arcan,lagod,haimad,autonomicd}.Dockerfile`: `COPY apps/ proto/` (workspace members + six build.rs proto inputs), pin `rust:1.93-bookworm`. Root cause of 3 weeks of FAILED deploys (since #1415 made `apps/bookkeeping-judge` a member). Validated by simulated-context `cargo metadata` + negative control reproducing the exact Railway error. |
| #1692 | `f7d18172` | arcan image ships `agents/` → prod boot log now `loaded 9 authored agent(s) from agents` (was: spawn_agent dead). |
| #1694 | `a2ca1a7e` | `arcan serve` wires `AgentAttestationAdapter` from `life init` identity (`--anima-identity-dir`/env/config, new `identity_loader`, corrupt identity hard-fails boot). 169 tests; adversarial review 0 must-fix. |
| #1696 | `0caa70bd` | `arcan agent test --live` (BRO-1008, last audit gap): canonical provider adapter chain, 50K-token `TokenBudgetHook` cap, `arcan::cost` module, double-gated live smoke. |
| #1697 | `6b5c4c74` | Client tool definitions threaded chat → lifegw ws → lifed → arcan-proxy → provider (`tool_definitions` additive proto fields; OpenAI `tools` array on gateway path; TOOL_CALL events on response side). Fixes the dogfooded "20 tools advertised, zero attached" gap. |
| #1695 | `d09257b0` | lifed per-substrate real/mock selection (honest boot summary) + lifegw-stack ships arcand on UDS. Security round: Tier-2 PEM scrubbed from arcan env (`env -u`), provider preflight skip+WARN instead of crash-loop, scoped `OPENAI_BASE_URL` (arcan appends `/v1` itself), trap guards unset pid. |

**Ops applied directly (Railway):** arcan service `LAGO_URL`
`:3001→:8080` (stale port; lagod binds 8080) + redeploy;
lifegw service `ARCAN_PROVIDER=openai` (`--skip-deploys`, rides the
#1695 rebuild).

## Lessons / process notes

1. **Dogfood-with-log-tracing finds what code review can't.** Five
   prod issues surfaced in ~30 min: 4 failing deploy lanes, mock
   substrates, dead tool attachment, custody 503, Redis-off — none
   visible from the repo alone.
2. **Subagent checkpoint discipline works.** The tools-dispatch
   implementation agent died on a session limit *after* two
   checkpoint commits — the work survived and merged as #1697. Two
   other agents died pre-commit in an earlier failed workflow run.
3. **`gh run watch --exit-status` lies about reruns** — it keys off
   the *run-level* conclusion, which stays `cancelled`/`failure` even
   when the rerun job passed (macOS flake confirmed passing on
   attempt 2 while the watcher reported failure). Check the job-level
   conclusion via `actions/runs/{id}/jobs?filter=latest`.
4. **Test (macOS) + Build Release only run on main pushes** — flaky
   tests in those lanes can't be pre-validated on PRs; merge-then-
   observe is the loop. (chronosd heartbeat flake: 200ms@40ms window;
   #1693 widens to 1s@20ms.)
5. **Same env var, two readers**: lifed wants `OPENAI_BASE_URL` with
   `/v1`, arcan-provider appends `/v1` itself. Co-located processes
   need scoped copies (entrypoint now strips for arcan).

## Pickup state (open threads ≤5)

- [ ] **Verify Stage-5 activation (the receipt):** lifegw-stack deploy
  at `d09257b0` boot log should show
  `substrates: arcan=real lago=mock haima=mock anima=mock` (railway
  logs --service lifegw). Then dogfood chat.broomva.tech: a multiturn
  conversation where the model actually calls a client tool (the
  #1697 wire) — watch for TOOL_CALL events in the stream. If arcan
  was skipped, check the preflight WARN (provider env).
- [ ] **Merge #1693** once Test (Linux) survives a runner (rebased on
  `d09257b0`; two consecutive runner-deaths so far, not test
  failures).
- [ ] **lago/haima/anima real substrates in lifegw-stack** — same
  pattern as arcan (Stage 6): add daemons to the container or decide
  TCP transport; lifed per-substrate selection (#1695) already
  supports incremental rollout.
- [ ] **Cross-repo (broomva.tech):** provision Redis (`REDIS_URL`) for
  resumable streams; investigate the Vercel-side custody mint 503
  (lifegw direct = 200); consider surfacing TOOL_CALL events in the
  chat UI if not already rendered.
- [ ] **Linear backfill** (MCP unauthenticated again): BRO-1008 done
  (#1696), attestation wiring done (#1694), plus the six PRs above.
  Nice-to-haves from reviews worth tickets: zeroize transient seed
  copies in identity_loader + life-cli read_seed; Railway
  watchPatterns missing `proto/**`/`apps/**`; lifegw-stack runtime
  skills set (skills_found=0 in prod arcan).

## Related context

- Prior handoffs: `2026-06-10-harness-phase2-and-backlog-stabilization.md`
  (Stage 2), `2026-06-10-chat-agent-grounding-shipped.md` (Stage 1).
- Dogfood receipt: `.context/dogfood-receipt-2026-06-10.md` (workspace
  nicosia, gitignored) — multiturn trace evidence incl. the model's
  "no tools attached" answer and the lifegw WS/JWT wire capture.
- Investigation + implementation ran as Workflow runs
  `wf_3b7a616d-183` (5 traces + adversarial verify) and
  `wf_37b210dc-69c` (4 worktree streams) — transcripts under the
  session dir if archaeology is needed.
