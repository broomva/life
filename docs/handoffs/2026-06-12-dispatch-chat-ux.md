# Dispatch chat UX — tool transcript, wrap-up, tool surface, loop fix

**TL;DR.** Four merged PRs took the chat write path from "denied at the
gate" (start of arc) to "executes once, renders its result, answers in
words, no wasted calls." BRO-1490 (workspace + ENOENT) is **closed and
prod-verified**; BRO-1465 (denial dead-air) and BRO-1466 (tool surface)
are **merged**; the loop-burn bug found in the post-fix receipt is
**merged** too. `main` @ `323926e8`. **FIRST ACTION: the final two-probe
receipt** on deploy `603f929d` (success path = exactly 2 model calls per
turn; denial path = a verbal explanation, not dead air) — then this arc
is fully closed and the next work is BRO-1471 (client-side) + tier
threading.

## State of the world (P15 snapshot 2026-06-12 ~late)

- **life** (this repo) — `main` @ `323926e8`. Workspace `riyadh`, branch
  `agent-tools-end-to-end` (rebased onto each squash-merge; clean).
- **Railway prod (lifegw-stack)** — deploy `603f929d` @ `323926e8`
  (in flight at handoff time; prior green deploy `87c871ab` @ `46638f3b`
  verified the transcript fix live). Boot receipt unchanged:
  `workspace=/var/life-state/arcan/workspace`, probe green,
  `substrates=arcan=real lago=real haima=mock anima=mock`.
- **broomva.tech (Vercel, separate repo)** — UNCHANGED; three findings
  now stacked on BRO-1471 (see Pickup).
- **No open PRs.** Local daemons: none.

## What this arc delivered (don't redo it)

| PR | Merge | What it gave |
|----|-------|--------------|
| #1742 | `1c279909` | `arcan serve --workspace` + boot writability probe + entrypoint writable volume-backed workspace. |
| #1744 | `880fd08b` | `FsPolicy::resolve_for_write` tolerates missing parent dirs (nearest-existing-ancestor boundary check); `LocalFs::write` resolves before `create_dir_all`. **The actual ENOENT.** |
| #1748 | `46638f3b` | **Tool transcript in conversation history** (`build_conversation_history` rendered NO tool events — model was blind to its own results) + **Recover wrap-up** (one continuation after denial/failure, BRO-1465) + **any-grant tool surface** wired into dispatch (`tools_allowed_by_policy` any-grant, not broad-only; `KernelRuntime::session_policy`; `substrate.rs` was `allowed_tools: None`) (BRO-1466). |
| #1751 | `323926e8` | **Loop continues on `tool_calls_executed > 0`, not `mode == Execute`** — the latter is also the homeostatic default for text-only ticks, so the post-#1748 receipt burned 4-5 wasted continuation model calls per turn. `TickOutput.tool_calls_executed` wired from the tick body's existing counter. |

Prior handoff (BRO-1490 close): `2026-06-12-agent-tools-end-to-end.md`.
Receipt (BRO-1490 green): `.context/dogfood-receipt-2026-06-12.md`.

## The receipt trail that drove #1748 + #1751

1. **13:50Z (`krzw3z…`, deploy `ccb3ec37`)** — BRO-1490 green:
   `write_file` executed, `bytes=27`, tick `Verify`. BUT the model
   re-called `write_file` **4×** across continuation ticks (it could not
   see the result) and the chat UI showed a contradictory "I can't run
   write_file here" (client's own no-tools completion).
2. **16:04Z (`vcz5i4zq…`, deploy `87c871ab` = #1748)** — transcript fix
   confirmed: `write_file` executed **once**, `bytes=16`, model answered
   in text (352 tokens). BUT the loop then fired **4 more text-only
   model calls** (`finish_reasons="stop"`, `mode=Execute`) before dying
   into Recover → #1751.
3. **Pending (deploy `603f929d` = #1751)** — expect: 1 tool tick + 1
   continuation = **2 model calls**, clean finish.

## Receipt status (2026-06-15) — kernel verified, fresh live probe BLOCKED on client

- **Kernel side: verified.** The transcript fix was empirically confirmed
  in prod at 16:04Z (`vcz5i4zq…`, deploy `87c871ab`/#1748): `write_file`
  executed exactly ONCE (the 4× repeat gone). The loop-burn fix (#1751)
  and the Recover wrap-up (#1748) are covered by e2e tests that assert
  the exact behaviors (`canonical_run_text_only_completion_ends_after_one_model_call`,
  `canonical_run_verbalizes_policy_denial_instead_of_dead_air`).
- **Fresh live two-probe receipt on `603f929d`: BLOCKED.** Across 4
  authenticated submits, the broomva.tech client created a session URL
  but showed "Something went wrong / An unexpected error occurred" and
  produced **zero** lifegw traffic (0 `tick finalized` since boot). The
  client's dispatch leg to the gateway is erroring — intermittently (it
  reached the gateway fine at 13:50Z and 16:04Z the same arc). This is a
  broomva.tech (Vercel, separate repo) regression, NOT the kernel
  changes; logged on BRO-1471 (comment 2026-06-15). When the client
  dispatches again, run the two probes below and confirm: probe 1 = 2
  model calls total (was 6); probe 2 = verbal denial, not dead air.

## The two-probe receipt (run when the client dispatches again)

Drive an AUTHENTICATED chat at broomva.tech/chat (the receipt REQUIRES
real auth — prod `dev_signer_enabled=false`, no API-direct path; the
session goes anonymous + errors if auth lapses, see Pickup):

```
# Probe 1 (success): "use your write_file tool to create artifacts/receipt.txt with the content 'ok'"
# Probe 2 (denial):  "use write_file to create /etc/evil.txt with the content 'x'"
railway logs --service lifegw | grep -E "file written|tool execution|tick finalized|finish_reasons|capabilities denied"
```

Expected on probe 1: ONE `praxis.fs … file written`, then ONE more tick
(`finish_reasons="stop"`) verbalizing it, then stop — **2 chat calls
total, not 6**; no further Execute ticks. Probe 2: `ToolCallFailed …
capabilities denied`, then ONE wrap-up tick that produces assistant text
(NOT `mode=Recover` as the terminal state with empty output).

Browser-driver note: extensions sleep when Chrome idles —
`osascript -e 'tell application "Google Chrome" to open location "<url>"'`
wakes the MV3 worker; then Interceptor (`interceptor tab new <url>`,
managed tabs only) or claude-in-chrome works. Both kept drifting to the
user's focused tab — pin a fresh managed tab.

## Pickup state (≤5 open threads)

- [ ] **Final receipt** (first action above) — closes this arc.
- [ ] **BRO-1471 (cross-repo, broomva.tech)** — now THREE stacked
  findings, all commented: (a) UI renders the client's own no-tools
  completion while the substrate executes tools (contradicts the
  runtime); (b) an expired auth session silently degrades to an
  erroring anonymous chat instead of prompting re-auth; (c) the visible
  reply path is the client's, not the dispatch stream's TOOL_CALL/
  TOOL_RESULT frames. Flip the client wire (#1697/#1714 server-ready) +
  render tool frames.
- [ ] **BRO-1466 second half** — dispatch has NO tier differentiation;
  every lifed-routed session gets `PolicySet::default()`. Thread the
  user's tier (lifegw claims → lifed → `CreateAgentReq` → arcand policy)
  so anonymous/free/pro see different surfaces. (Visibility logic + the
  any-grant fix are done; only tier propagation remains.)
- [ ] **BRO-1491** — per-session workspace isolation. Kernel already
  writes per-session `workspace_root=/var/life-state/arcan/sessions/<sid>`
  and threads it in every `ToolExecutionRequest`; the praxis FsPort is
  still construction-time-fixed. Full design notes on the issue.
- [ ] Backlog: BRO-1480 (blob MIME durability), BRO-1481 (same-second
  same-size edits), Topology-A HTTP branch param, Stage 7 (haima/anima
  real substrates). Also: the `opsis_*` skill tools log "unknown tool"
  WARNs on every dispatch tick — a skill references tools not in the
  registry (harmless noise, worth a cleanup ticket).

## Key code (this arc)

- Tool transcript: `aios-runtime/src/lib.rs::build_conversation_history`
  (+ `append_tool_line`, `truncate_for_history`, the budget consts).
- Recover wrap-up: `arcand/src/substrate.rs` dispatch loop +
  `arcand/src/canonical.rs` `run_session` loop (both flag-guarded).
- Loop predicate: `TickOutput.tool_calls_executed` (`aios-runtime`),
  consumed at both loop sites above.
- Tool surface: `arcan-aios-adapters/src/capability_map.rs`
  (`tools_allowed_by_policy`, `allows_category`) +
  `KernelRuntime::session_policy` + `substrate.rs` `allowed_tools`.
- Tests: `aios-runtime/tests/tool_history.rs` (4),
  `arcand/tests/canonical_api.rs` (`…_verbalizes_policy_denial…`,
  `…_text_only_completion_ends_after_one_model_call`).
