# Lago as the FS substrate + dispatch branching — Stage 5

**TL;DR.** Continuation of the same-day Stage-4 handoff. The user named
the architecture ("lago should be the FS substrate of Life agents") and
this arc shipped it end-to-end, then went one further: **every way a
public-session agent touches its filesystem — file tools, blob bytes,
shell mutations — now flows into the durable, content-addressed,
replayable lago store, and a dispatch can fork the session onto a
branch.** Five PRs merged (#1724 journal→lagod, #1725 remote blobs,
#1726 exec-path reconcile, #1730 capability namespace fix, #1733
branch-through-dispatch with auto-fork). Prod verified at each stage;
final rebuild (`9a65faad`) rolling at handoff time. **FIRST ACTION:**
verify the `9a65faad` boot (routine — watcher pattern in §Pickup), then
the one open ribbon: the live chat write-receipt (needs Chrome).

## State of the world (P15 snapshot 2026-06-11 ~17:30 -05)

- **main** @ `9a65faad`. Merged this arc, in order: #1724 (`b23f3d78`
  Stage 6b), #1725 (`9df8dc80` remote blobs), #1726 (`6b015942`
  exec-path), #1730 (`0836cc05` capability namespace), #1733
  (`9a65faad` branching). Plus the Stage-4 handoff #1718 (`152905d0`).
- **OPEN PRs: none.** All review threads closed (verdict comments on
  each PR map finding → fix).
- **Railway prod**: lifegw-stack SUCCESS at `0836cc05`; `9a65faad`
  BUILDING at handoff. Boot receipt shape (verified at `6b015942` +
  `0836cc05`):
  `arcan: Starting arcan (remote Lago journal) lago_url=http://127.0.0.1:8077`
  · `arcan blob content: remote lago blob store` ·
  `substrates=arcan=real lago=real haima=mock anima=mock`.
- **Linear**: BRO-1476/1477/1478/1479 Done. Backlog: BRO-1465/1466
  (denial UX + tool-surface pre-filter), BRO-1480 (blob store blocks
  executable MIME — durability policy question), BRO-1481 (same-second
  same-size edits missed by reconcile fast-path), BRO-1471 (chat client:
  flip on tool sending + render TOOL_CALL frames; the UI literally
  labels its 20 tools "preview — not wired into the live runtime yet").
- **Workspace** `manila`, synced to main. All agent worktrees pruned.

## What this arc delivered (don't redo it)

| PR | Merge | What it gave |
|----|-------|--------------|
| #1724 | `b23f3d78` | **Stage 6b**: arcan's event journal → in-container lagod over HTTP (`LAGO_URL=http://127.0.0.1:8077`; entrypoint reordered lagod-first + HTTP-plane probe; `ARCAN_LAGO_URL=embedded` escape; arcan data dir → volume). Killed the two-lago-instances-side-by-side state where agent events died on redeploy. |
| #1725 | `9df8dc80` | **Remote blobs**: `BlobBackend` trait (lago-store) + `RemoteBlobBackend` (arcan-lago) over lagod `/v1/blobs/{hash}`; selected by the same `LAGO_URL` switch. The async↔sync bridge is a **dedicated worker thread** — an inline `block_on` panics on a Tokio worker (the tool harness runs the sync tool chain with no `spawn_blocking`); pinned by a from-within-a-runtime round-trip test. Review round also fixed: missing HTTP timeouts (worker-funnel starvation), a server `parse_range` u64 **underflow on zero-byte blobs** (416 path; client now reads 416 as present-but-empty), and durability-loss being logged at debug. |
| #1726 | `6b015942` | **Exec-path reconcile**: `ReconcilingTool` wraps bash; post-exec bounded workspace diff → blobs + manifest + FileWrite/FileDelete through the same channel as FsPort writes. Review round fixed two journal-corruption must-fixes — **baseline-at-boot** (`FsTracker::with_baseline`; empty manifest vs populated CWD flooded the journal with a FileWrite per pre-existing file on first exec) and **phantom dir-sentinel FileDeletes** — plus symlink boundary escape, lock held across the O(n) walk, nondeterministic cap truncation. |
| #1730 | `0836cc05` | **Capability namespace fix** (live-dogfood find): every fs tool call from public sessions was denied — capabilities derived from raw workspace-relative paths (`fs:write:artifacts/x`) never matched the `/session/`-rooted policy patterns. `policy_path()` anchors relative paths under `/session/`; traversal + absolutes stay raw (denied). FsPolicy remains the execution-time boundary (defense in depth). |
| #1733 | `9a65faad` | **Branching through dispatch** (BRO-1479): additive `branch` on `life.v1.SendMessageReq` + `arcan.v1.DispatchMessageReq`, threaded lifegw ws → lifed → arcan-proxy → arcand. The substrate **auto-forks an unknown branch from main at the current head** via the kernel's existing `create_branch` (real fork: parent, fork_sequence, head tracking; `BranchCreated` is the fork's first event) — the naive "just key the tick" approach journals NOTHING (`next_sequence` bails "branch not found"; found by e2e, traced to the kernel's real branch lifecycle). Validation `[a-zA-Z0-9_-]{1,64}` at BOTH boundaries (lifegw closes `policy_violation:invalid_branch`; arcand INVALID_ARGUMENT); idempotent fork under concurrent dispatch races; pump filters session+branch. Empty ⇒ main, pinned. Deferred: Topology-A HTTP route, merge wire-params, client UI. |

## Lessons / process notes (this arc's additions)

1. **Five agent session-deaths, zero lost work** — checkpoint commits +
   worktree-resume briefs held every time. Two dying agents shipped
   latent production bugs (the blob `block_on` panic, the exec-path
   journal flooding) that adversarial review + in-session root-cause
   tracing caught pre-merge. The pattern is load-bearing, not luck.
2. **A PR with a merge conflict never runs `pull_request` CI** — GitHub
   can't build the merge ref; checks sit absent (not failed) and
   close/reopen + empty commits do nothing. Diagnose with
   `gh pr view --json mergeable` (CONFLICTING/DIRTY), resolve, then CI
   fires.
3. **Live dogfooding keeps out-finding code review** — the capability
   namespace mismatch (#1730) was invisible to every reviewer across
   five PRs because the bug lived BETWEEN two correct layers (praxis
   relative paths vs policy virtual namespace). One real chat message
   found it in seconds.
4. **The kernel often already has the primitive** — branching needed no
   new substrate: `create_branch`/`merge_branch`/branch-keyed
   sequencing existed, unexposed. Investigate before building.
5. `cargo check --workspace --all-targets` is the only honest workspace
   gate (third+ time test-target-only breaks reached CI without it).

## Pickup state (open threads ≤5)

- [ ] **Verify the `9a65faad` deploy** (watcher was running at handoff):
  boot log keeps the §State receipt shape; healthz OK. Routine.
- [ ] **The live write receipt (needs Chrome):** chat
  "use write_file to create artifacts/receipt.txt …" → with #1730
  deployed the call now passes policy → tracked write → **remote blob
  PUT + FileWrite append into lagod**. Optionally follow with a
  `branch`-carrying dispatch once the chat client can send it
  (BRO-1471). Logs: `railway logs --service lifegw` around the turn —
  no `capabilities denied`, no Recover; tick `finish_reasons` →
  tool execution → clean finish.
- [ ] **BRO-1465/1466** — model still advertises tools the tier denies
  (the `tools_allowed_by_policy` pre-filter exists, unwired into the
  dispatch's `allowed_tools`) and denial still ends turns as dead air.
  Highest-leverage chat-UX work in this repo.
- [ ] **Cross-repo (broomva.tech, BRO-1471):** flip client tool sending
  on (the wire is ready: #1697 + #1714), render TOOL_CALL frames,
  branch selector UI when wanted.
- [ ] **Stage 7 (haima/anima real substrates)** — same incremental
  pattern; haimad needs wallet/chain env decisions first.

## Related context

- Prior handoffs (same day): `2026-06-11-stage6-client-tools-ci-hardening.md`
  (Stage 4), `2026-06-10-prod-restoration-and-tool-wire.md` (Stage 3).
- Dogfood receipt: `.context/dogfood-receipt-2026-06-11.md` (workspace
  manila, gitignored) — §7 carries the FS-substrate completion evidence.
- Cross-review verdicts: PR comments on #1725, #1726, #1733.
