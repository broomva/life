# Agent tools end-to-end — BRO-1490 closed, fresh-session pickup

**TL;DR.** The file-tool write path is **validated end-to-end in prod**:
chat `write_file` → policy gate → resolve → disk → lago tracking → clean
tick (receipt 2026-06-12 13:50Z, session `krzw3zpmzm2nfqyqjjfcp2hfge`,
`file written bytes=27`, tick `mode=Verify`, zero failures). BRO-1490
turned out to be TWO stacked bugs — the root-owned workspace (#1742) and
a `resolve_for_write` missing-parent ENOENT (#1744) — both merged and
live @ `880fd08b`. **FIRST ACTION: BRO-1466/1465 (tool-surface + denial
wrap-up)** — read the scoping comments on the issues first; a naive
`tools_allowed_by_policy` wiring would REGRESS the receipt path.

## State of the world (P15 snapshot 2026-06-12 ~09:00 -05)

- **life** (this repo) — `main` @ `880fd08b`. Workspace `riyadh`, branch
  `agent-tools-end-to-end` (reset onto main between the two PRs; holds
  only uncommitted docs at handoff time). BRO-1490 **Done** in Linear.
- **Railway prod (Life project, lifegw-stack)** — deploy `ccb3ec37` @
  `880fd08b` SUCCESS, healthz OK. Boot receipt:
  `workspace=/var/life-state/arcan/workspace` (writable, probe green) ·
  remote lago journal + remote blobs · `substrates=arcan=real lago=real
  haima=mock anima=mock` · 9 authored agents.
- **broomva.tech (Vercel, separate repo)** — UNCHANGED and now provably
  misleading: during the green receipt the UI rendered its own no-tools
  completion (*"I can't run write_file here…"*) while the substrate
  executed the tool. BRO-1471 commented with the evidence.
- **No open PRs, no unresolved review threads.** Local daemons: none.

## What this arc delivered (don't redo it)

| PR | Merge | What it gave |
|----|-------|--------------|
| #1742 | `1c279909` | `arcan serve --workspace <DIR>` (env `ARCAN_WORKSPACE_DIR` — `ARCAN_WORKSPACE` was taken by hooks); boot-time writability probe (WARN + remedy); entrypoint §3b creates + chowns `${ARCAN_DATA_DIR}/workspace` (volume-backed) and passes `--workspace`; README updated (incl. stale BRO-1478 note fixed). |
| #1744 | `880fd08b` | **The actual ENOENT fix**: `FsPolicy::resolve_for_write` now walks to the nearest EXISTING ancestor, boundary-checks there, re-appends missing components (rejects `..`/`.` in the not-yet-existing suffix). `LocalFs::write` resolves BEFORE `create_dir_all` (old order manufactured dirs for unchecked paths). 10 new tests incl. the exact prod regression (`write_file artifacts/receipt.txt` in an empty workspace). |

Receipt: `.context/dogfood-receipt-2026-06-12.md` (riyadh workspace).
Supersedes: `2026-06-11-agent-tools-end-to-end.md`.

### Why two bugs looked like one

The 02:44Z re-receipt after #1742 failed with the IDENTICAL
`io error: No such file or directory` — the tool's pre-flight
`resolve_for_write` canonicalized a parent that must exist
(`artifacts/` doesn't, in a fresh workspace), failing BEFORE
`LocalFs::write`'s `create_dir_all`. Tests never caught it because every
write-tool test writes at the workspace root. The bare `io error:` (no
`Failed to write file:` prefix) pinned the failing call. Lesson recorded:
error-prefix forensics beat assumption; the handoff's single-cause
diagnosis cost one deploy cycle.

## First action — BRO-1466 + BRO-1465 (tool-surface + denial UX)

Read the issue comments FIRST (both have code-verified scoping from this
arc, 2026-06-11/12):

- **BRO-1466 (tool surface)** — `substrate.rs:394` hardcodes
  `allowed_tools: None`; `canonical.rs:1462` already derives it.
  **TRAP**: `tools_allowed_by_policy` counts only BROAD grants
  (`fs:write:*`), but dispatch sessions get path-scoped
  `PolicySet::default()` (`fs:write:/session/artifacts/**`, `exec:git`) —
  wiring it as-is would HIDE write_file and regress the receipt. Fix
  shape: any-grant-based visibility (gate still enforces scope at
  execution), then thread at `substrate.rs:394`. Bigger second half:
  dispatch has NO tier differentiation (every lifed-routed session gets
  the default policy; tier threading gw→lifed→CreateAgentReq→policy).
- **BRO-1465 (denial dead-air)** — `aios-runtime/lib.rs:1091-1117`:
  denial ⇒ `mode=Recover` ⇒ both dispatch loops break with no wrap-up
  call. Fix shape: ONE wrap-up iteration on Recover (flag-guarded,
  Recover→Recover breaks), both `substrate.rs` and `canonical.rs`.
  **Verify first**: does the context compiler render `ToolCallFailed`
  into provider messages? (If not, the wrap-up call sees nothing.)
  Related fresh evidence: gpt-5-mini re-called write_file 4× across
  continuation ticks even on SUCCESS — tool-result legibility in
  reconstructed history may be weak for the dispatch path generally.

## Pickup state (≤5 open threads)

- [ ] **BRO-1466 / BRO-1465** — first action above. Highest-leverage
  chat UX; both scoped on the issues.
- [ ] **BRO-1471 (cross-repo, broomva.tech)** — now urgent-ish: the UI
  actively contradicts the runtime (renders its own no-tools completion
  while the substrate executes tools). Flip the client wire (#1697 +
  #1714 server-side ready) + render TOOL_CALL/TOOL_RESULT frames.
- [ ] **BRO-1491** — per-session workspace isolation (kernel already
  threads `manifest.workspace_root` per request; adapter drops it;
  full design notes + acceptance criteria on the issue). Note the
  kernel session log now shows per-session
  `workspace_root=/var/life-state/arcan/sessions/<sid>` — the dirs
  exist and are writable; only the praxis FsPort rebind is missing.
- [ ] **Branching client UX** — `branch` wire-ready (#1733); fresh chat
  could expose a branch selector / fork button.
- [ ] Backlog (no action): BRO-1480 (blob MIME durability policy),
  BRO-1481 (same-second same-size edits), Stage 7 (haima/anima real),
  Topology-A HTTP branch param (`canonical.rs` still `BranchId::main()`).

## Operational notes for the next dogfood

- Browser drivers (claude-in-chrome MCP, Interceptor) lose their native
  host when Chrome idles overnight. Wake them WITHOUT restarting Chrome:
  `osascript -e 'tell application "Google Chrome" to open location
  "<url>"'` — the tab event wakes the MV3 service workers; then
  `interceptor tabs` works.
- Receipt greps (lifegw is the one service; lagod logs only boot lines):
  `railway logs --service lifegw | grep -E "write_file|file written|FileWrite|NOT tracked|Recover|tick finalized"`.
  Positive signals: `file written bytes=N` + `life.tool.status="ok"` +
  tick mode ≠ Recover. Silent-failure checks: `NOT tracked` (blob),
  `failed to append event` (journal) — both must be zero.
- `dev_signer_enabled = false` in prod lifegw.toml — there is no
  API-direct token path; the receipt REQUIRES the authed browser.

## Related context

- Receipt: `.context/dogfood-receipt-2026-06-12.md` (riyadh)
- Prior arc: `docs/handoffs/2026-06-11-agent-tools-end-to-end.md`
  (superseded), `2026-06-11-lago-fs-substrate-and-branching.md` (Stage 5)
- Linear: BRO-1490 (Done) · BRO-1491 (new) · BRO-1465/1466/1471
  (scoping comments) · BRO-1479/1480/1481
- Key code: `capability_map.rs` (tool surface), `substrate.rs:394`
  (dispatch allowed_tools), `aios-runtime/lib.rs:1091` (denial path),
  `praxis-core/src/workspace.rs` (resolve_for_write)
