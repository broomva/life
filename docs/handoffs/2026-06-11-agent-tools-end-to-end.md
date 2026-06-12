# Agent tools end-to-end — fresh-session pickup

**TL;DR.** The lago FS-substrate arc + dispatch branching are merged and
live in prod (`main` @ `4f9858e7`, `arcan=real lago=real`, remote
journal + remote blobs); a live Chrome dogfood validated the #1730
capability fix (public `write_file` now **passes the policy gate** — no
more dead-air denial) but surfaced the next blocker one layer down.
**FIRST ACTION: fix BRO-1490** — the agent's file-tool workspace is the
root-owned image `WORKDIR /opt/life`, so the write fails ENOENT before
it can reach lago. Patch the deploy/wiring, redeploy, re-run the
two-line chat receipt, then move to the tool-surface/UX work
(BRO-1465/1466/1471).

## State of the world (P15 snapshot 2026-06-11 ~20:30 -05)

- **life** (this repo) — `main` @ `4f9858e7`. Workspace `manila`,
  current branch `feature/bro-1490-arcan-agent-file-tool-workspace-is-the-root-owned-image`
  (created by `linear issue --start`; holds this handoff). Clean tree.
- **Railway prod (Life project, lifegw-stack)** — SUCCESS @ `4f9858e7`,
  healthz `OK`. Boot receipt:
  `arcan: Starting arcan (remote Lago journal) lago_url=http://127.0.0.1:8077`
  · `arcan blob content: remote lago blob store` ·
  `substrates=arcan=real lago=real haima=mock anima=mock`.
- **broomva.tech (Vercel, separate repo)** — chat client unchanged; its
  Tools panel still labels the 20 tools "preview — not wired into the
  live runtime yet" (the SERVER wire is ready; the client hasn't flipped
  it on — BRO-1471).
- **No open PRs, no unresolved review threads, no running daemons.**

## What the FS-substrate + branching arc delivered (don't redo it)

| PR | Merge | What it gave |
|----|-------|--------------|
| #1724 | `b23f3d78` | Stage 6b — arcan event journal → in-container lagod over HTTP (`LAGO_URL`); volume-backed; `ARCAN_LAGO_URL=embedded` escape. |
| #1725 | `9df8dc80` | Remote blobs — `BlobBackend` trait + `RemoteBlobBackend` (dedicated-thread async↔sync bridge; fixed a `block_on`-on-Tokio-worker panic, a server `parse_range` u64 underflow, timeouts, durability logging). |
| #1726 | `6b015942` | Exec-path reconcile — shell writes → manifest/blobs/journal (baseline-at-boot; dir-sentinel + symlink + lock + cap fixes). |
| #1730 | `0836cc05` | **Capability namespace fix** — relative fs-tool paths anchor under `/session/` so the default policy's `fs:write:/session/artifacts/**` matches. **Validated live** (see receipt below). |
| #1733 | `9a65faad` | Branching through dispatch — `branch` field threaded gw→lifed→proxy→arcand; **auto-fork from main at head** via kernel `create_branch`; validated both boundaries; empty ⇒ main. |

Full narrative: `docs/handoffs/2026-06-11-lago-fs-substrate-and-branching.md`.

## The dogfood that produced this handoff (the proof + the new finding)

Real Chrome, authed chat at `broomva.tech/chat`, session
`wzkjw7cfkds7huw2psiuzc5wyq`, message: *"use your write_file tool to
create artifacts/receipt.txt …"*. Server log:

```
chat … gen_ai.response.finish_reasons="tool_calls"          ← model proposed write_file
WARN aios_runtime: tool execution failed (write_file):
     io error: No such file or directory (os error 2)        ← the new bug
tick finalized mode=Recover
```

- ✅ **#1730 validated**: the write_file call was **NOT denied** — no
  `capabilities denied`, the dead-air-by-policy is gone. The gate now
  reaches execution. This was the thing #1730 targeted.
- ❌ **BRO-1490 found**: the write fails at the filesystem layer, so it
  never reaches the tracker → no blob PUT / FileWrite. The end-to-end
  lago receipt is blocked here, not at policy.
- Receipt: `.context/dogfood-receipt-2026-06-11.md` §9.

## First action — BRO-1490 (unblocks the whole file-tool path)

**Root cause (code-verified):** `crates/arcan/arcan/src/main.rs:642`
sets the praxis tools' `FsPolicy` workspace_root = `std::env::current_dir()`
= the image `WORKDIR /opt/life` (prod tick log confirms `workspace=/opt/life`).
`deploy/railway/lifegw-stack/Dockerfile` creates `/opt/life` via
`WORKDIR` + `COPY agents/` **as root** and never `chown`s it to the
`life` runtime user; arcan runs as `life`. So `write_file` (which
`create_dir_all`s the parent then writes, `local_fs.rs:52-65`) cannot
create/write under root-owned `/opt/life`. Two problems in one:
1. **not writable** by the agent user, and
2. **shared across all sessions** — the kernel's per-session
   `manifest.workspace_root` is ignored by the construction-time-fixed
   praxis `FsPort` (a multi-tenancy isolation gap).

**Pick the smaller fix first (deploy-only, fast receipt):** give arcan a
writable workspace. In `deploy/railway/lifegw-stack/entrypoint.sh` §3b
(the arcan block), create + `chown life:life-runtime` a workspace dir
under the volume (e.g. `${LIFE_STATE_DIR}/arcan/workspace`) and start
arcan with that as CWD (or add an `ARCAN_WORKSPACE`/`--workspace` flag —
check `main.rs` arg parsing; `current_dir()` is the only source today).
Redeploy, then re-run the receipt:

```
# chat.broomva.tech: "use write_file to create artifacts/receipt.txt with 'hi'"
railway logs --service lifegw | grep -E "write_file|FileWrite|blob|Recover|tick finalized"
# Expected: NO "tool execution failed"; a FileWrite event + remote blob PUT;
#           tick finishes clean (not mode=Recover).
```

**Then do the proper fix (follow-up, closes the isolation gap):** thread
the kernel's per-session `manifest.workspace_root` into praxis tool
execution so each session writes to its own dir. `ArcanHarnessAdapter::execute`
(`crates/arcan/arcan-aios-adapters/src/tools.rs`) currently builds a
`ToolContext` without the workspace; the request already carries it.

## Pickup state (≤5 open threads)

- [ ] **BRO-1490** — agent workspace not writable (first action above).
  Quick deploy fix → receipt; proper per-session fix → follow-up.
- [ ] **BRO-1465 / BRO-1466** — tool-surface + denial UX. `tools_allowed_by_policy`
  (`crates/arcan/arcan-aios-adapters/src/capability_map.rs`) exists but
  isn't wired into the dispatch's `allowed_tools`, so the model still
  advertises tools the tier denies; and a denial still ends the turn as
  dead-air `Recover` with no verbal wrap-up. Highest-leverage chat UX.
- [ ] **BRO-1471 (cross-repo, broomva.tech)** — flip client tool-sending
  ON (the server wire is ready: #1697 + #1714) and **render
  TOOL_CALL/TOOL_RESULT frames** instead of empty bubbles (the chat UI
  shows nothing for tool-only turns — this masked both the receipt and
  the dogfood evidence; the server log was the only signal).
- [ ] **Branching client UX** — `branch` is wire-ready end-to-end
  (#1733); a fresh chat could expose a branch selector / fork button.
- [ ] Backlog (no action now): BRO-1480 (blob store blocks executable
  MIME — durability policy Q), BRO-1481 (same-second same-size edits),
  Stage 7 (haima/anima real substrates).

## Related context

- Dogfood receipt (with §9 finding): `.context/dogfood-receipt-2026-06-11.md`
- Prior handoffs (same day): `2026-06-11-lago-fs-substrate-and-branching.md`
  (Stage 5), `2026-06-11-stage6-client-tools-ci-hardening.md` (Stage 4).
- Linear: BRO-1490 (new), BRO-1465/1466/1471/1479/1480/1481.
- Capability + tool-surface logic: `crates/arcan/arcan-aios-adapters/src/capability_map.rs`.
