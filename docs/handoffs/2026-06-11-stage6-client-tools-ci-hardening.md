# Stage 6 + kernel client tools + CI hardening — Stage 4

**TL;DR.** Continuation of the Stage-3 handoff's pickup list — all five
items closed or advanced. **Stage-5 receipt verified in prod** (real
arcand over UDS at current main), then the dogfood exposed that the
Stage-5 activation had **regressed chat client tools**: arcand's
kernel dispatch received `DispatchMessageReq.tool_definitions` and
dropped them (the documented substrate.rs follow-up) — the gateway
path #1697 fixed was no longer the path prod ran. This session closed
that gap (#1714), shipped **Stage 6: real lagod inside lifegw-stack**
(#1713, prod boot now `substrates: arcan=real lago=real haima=mock
anima=mock`), and fixed two CI killers: the Test (Linux) runner-death
(#1700, disk exhaustion) and a lago-journal test flake (#1715). **Four
PRs merged**, two adversarial review rounds applied, Linear backfilled
(BRO-1461..1471). **FIRST ACTION:** the one open receipt — live
browser dogfood of chat.broomva.tech tool calls (needs Chrome with the
extension; everything wire-level is already CI-pinned).

## State of the world (P15 snapshot 2026-06-11 ~10:00 -05)

- **main** @ `813c4ba5`. Merged this session, in order: #1700
  (`9907be82` CI runner-death), #1715 (`ccaa65ab` lago test flake),
  #1713 (`7312e92a` Stage 6 lagod), #1714 (`813c4ba5` kernel client
  tools).
- **OPEN PRs: none.** No open review threads; both feature PRs carry
  cross-review verdict comments + fix rounds.
- **Railway prod (Life project)**: lifegw-stack SUCCESS at `813c4ba5`,
  healthz OK, boot line `substrates: arcan=real lago=real haima=mock
  anima=mock`; lagod journal/blobs persist on the volume
  (`/var/life-state/lago`), internal ports pinned (grpc 50051, http
  8077 — lagod's HTTP default 8080 collided with Caddy's `$PORT`).
  The four substrate services (arcan/lagod/haimad/autonomicd) still
  run their last-good builds (SKIPPED on docs/chronos-only merges —
  watchPatterns, BRO-1467).
- **Linear (workspace `broomva`, team BRO)**: BRO-1461 umbrella
  (Done), BRO-1462 CI fix (Done), BRO-1463 client tools (Done),
  BRO-1464 Stage 6 (Done), BRO-1465..1471 backlog (denial dead-air UX,
  policy/tool-surface alignment, watchPatterns, zeroize seeds, runtime
  skills, arcand hardening, chat-UI TOOL_CALL rendering + Redis +
  custody 503).
- **Conductor workspace** `manila`, branch `prod-restoration-tool-wire`
  (synced to main) + this handoff branch. Agent worktrees pruned
  (disk was at 100% mid-session; ~64 GB freed — stale `wf_37b210dc-*`
  trees and agent `.target` dirs).

## What this session delivered (don't redo it)

| PR | Merge | What it gave |
|----|-------|--------------|
| #1700 | `9907be82` | Test (Linux) runner-death fix: NULL step conclusions on 3+ main pushes = disk exhaustion (125-crate debug+test build + bloated v0 cache > ~14 GB runner disk). `CARGO_PROFILE_{DEV,TEST}_DEBUG=0`, ~10 GB preinstalled bloat shed, rust-cache lineage v0→v1, df probes. Lane passed 9m40s cold — faster than the old green runs. |
| #1715 | `ccaa65ab` | `lago_events_track_writes` flake: background writer (mpsc → sync redb commit) raced a fixed 200 ms sleep; now a 5 s deadline poll. Same family as the #1693 chronos window. |
| #1713 | `7312e92a` | Stage 6: lagod gains a substrate-plane UDS server (`--uds-socket`/`LAGO_UDS_SOCKET` — BRO-1017 left `lago.v1.LagoSubstrate` TCP-only while lifed's lago-proxy dials UDS, so real lago was unselectable). lifegw-stack builds+ships+boots lagod before lifed; volume-backed data; port pinning; **review round**: entrypoint `set -u` crash guard (`${OPENAI_BASE_URL:-}` — ANTHROPIC-only configs died in §3), live shutdown trap (`exec caddy` had erased it — caddy now backgrounded under trap + double-wait), lagod eager UDS bind (fail fast, not log-and-continue), stale-socket rebind test. |
| #1714 | `813c4ba5` | Kernel client tools (completes the #1697 arc on the real-substrate path): `ClientToolDefinition` (strict parse — mistyped fields reject, absent fields permissive) rides `TickInput.client_tools` → provider request on every tick; model proposal of a client tool → `ToolCallRequested(category="client")` → wire `TOOL_CALL_PENDING`, no kernel policy/harness, clean `Sleep` turn end (client executes, continues via replayed history). Registry-wins collisions (full-registry dedup in the adapter), trust-boundary caps (≤64 defs, ≤16 KB/entry, name `[a-zA-Z0-9_-]{1,64}`), **review round**: AskHuman no longer clobbered in mixed approval+client completions; `with_registry_tool_names` additive + foot-gun documented; e2e wires registry names like `arcan serve`. +12 tests on the arc incl. a topology-B e2e pinning defs → provider → TOOL_CALL_PENDING(category=client) → FINISH, no TOOL_RESULT, exactly one provider call. |

**Ops/process:** Linear CLI works (`linear -w broomva`, team BRO — note
the codebase's old BRO-1xxx doc references don't map to this Linear's
numbering; recent 14xx tickets do). codex CLI quota-limited → P20 ran
as Strata-B fresh-context reviewers (verdicts on both PRs were
substantive: 1 must-fix + 4 should-fix each, all real).

## Lessons / process notes

1. **`cargo check --workspace` does NOT compile test targets.** The
   TickInput field addition broke arcan-ergon's tests on CI while all
   local gates were green. The workspace gate is `cargo check
   --workspace --all-targets`.
2. **Subagent session-limit deaths are the norm for >5-min legs** —
   three Engineer agents died mid-task this session (~100-170K
   subagent tokens each). The resume pattern works: checkpoint
   commits + fresh agent pointed at the dead agent's worktree with an
   inherited-state brief. Budget ~150K tokens per leg; scope briefs
   accordingly.
3. **Fixed sleeps in tests are landmines** — two found within hours
   (chronos #1693's window, lago journal #1715). Deadline-poll instead.
4. **`gh pr checks --watch | tail` eats the exit code** (pipeline exit
   = tail's). Check conclusions explicitly; never trust the pipe.
5. **Adversarial cross-review pays for itself**: the #1713 reviewer
   found a container-crash path reachable via the PR's own documented
   smoke command; the #1714 reviewer found the AskHuman clobber and
   the allowed_tools/full-registry dedup mismatch. None were visible
   to the writing context.
6. **Same-anchor STATUS.md entries conflict on the second merge** —
   expected; resolve by keeping both entries, one anchor.

## Pickup state (open threads ≤5)

- [ ] **Live dogfood receipt (the one open item):** Chrome+extension
  session on chat.broomva.tech — (1) ask the model to list tools: with
  the Tools-menu defaults it should now list client tools alongside
  the 14 registry tools (pre-#1714 it listed only the registry set);
  (2) trigger a client tool and watch the stream for the TOOL_CALL
  event; (3) `railway logs --service lifegw` should show the INFO
  `client tool definitions parsed` line + `finish_reasons="tool_calls"`
  + tick mode Sleep (not Recover). Note: the UI may render tool-call
  turns as an empty bubble (BRO-1471, cross-repo) — the logs + WS
  frames are the receipt either way.
- [ ] **Stage 7 (haima/anima real substrates)** — same pattern as
  Stage 6; haimad needs wallet/chain env decisions first (BRO-1144
  neighborhood), anima custody is the soma shape.
- [ ] **Registry-denial dead-air UX (BRO-1465/1466)** — bash & friends
  still advertised to chat sessions but denied by default policy;
  model gets no wrap-up call after denial. Highest-leverage chat-UX
  fix in this repo.
- [ ] **Cross-repo (broomva.tech, BRO-1471):** render
  TOOL_CALL/TOOL_RESULT stream parts (empty bubble today), provision
  `REDIS_URL`, custody mint 503.
- [ ] **CI follow-through:** watch the next few Test (Linux) runs for
  disk headroom (df probes are in the job log); if the v1 cache grows
  the same way, consider a scheduled cache eviction or lane split.

## Related context

- Prior handoff: `2026-06-10-prod-restoration-and-tool-wire.md`
  (Stage 3 — its five pickup items drove this session).
- Dogfood receipt: `.context/dogfood-receipt-2026-06-11.md` (workspace
  manila, gitignored) — Stage-5 boot evidence, the two-turn chat trace
  that exposed the client-tools regression and the policy-denial
  dead-air, CI forensics.
- Cross-review verdicts: PR comments on #1713 and #1714 (both
  "verdict applied" comments enumerate finding → fix mapping).
