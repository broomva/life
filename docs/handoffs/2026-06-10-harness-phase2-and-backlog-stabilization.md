# Harness Phase-2 shipped + PR backlog stabilization — Stage 2

**TL;DR.** The harness Phase-2 arc is **fully shipped and merged**: the
substrate wire carries the tool lifecycle end-to-end (life #1686), and
both audited ergon Workflow-tick gaps are closed — durable lago sinks +
three real auto-hook adapters (life #1687). The chat surface's
remaining ungrounded path (tool events) is now one ops step away from
the browser: deploy arcand beside lifed and flip
`LIFED_ARCAN_BACKEND`. The same session also drained the PR backlog
13 → 3 (10 merged, 3 superseded-with-reason), including the
long-stale dependabot majors (rig-core 0.36 + rmcp 1.7 migrated;
lance 0.28 attempted, reverted — breaks Linux clippy inside the dep)
and the `life init` Anima identity bootstrap (#1242). **FIRST
ACTION:** confirm life #1689 (repair PR: deny licenses + lance revert)
merged green; if its CI settled while no agent was watching, merge it
— main's Dependency Check + Lint lanes are red until it lands.

## State of the world (P15 snapshot 2026-06-10 ~17:00 -05)

- **broomva/life `main`** @ `3164a851` (tracing-appender bump). Twelve
  merges landed today (see table). Production grounding (Stage 1)
  verified live on broomva.tech earlier today.
- **OPEN PRs (3):**
  - **#1689** — `fix(ci)`: repair Dependency Check (allow
    `Apache-2.0 WITH LLVM-exception`, kept dormant) + Lint (revert
    lance 0.28→0.24). **Merge this first** — main is red on those two
    non-required lanes until it lands.
  - **#1041** (bitflags 2.13) + **#1039** (axum 0.8.9) — dependabot
    re-rebasing after lock conflicts; merge when CLEAN + green.
- **Railway/Vercel prod**: unchanged since Stage 1 — lifegw-stack
  serves the grounded persona via `vercel_ai_gateway`; arcand NOT yet
  in the prod chat path.
- Conductor workspace `asuncion`; local branches pruned except
  in-flight ones.

## What this session delivered (don't redo it)

| PR | Merge | What it gave |
|----|-------|--------------|
| #1686 | `14e39f3c` | **Substrate tool lifecycle (Phase 2)**: `proto/arcan/v1/substrate.proto` +TOOL_CALL_PENDING/+TOOL_RESULT/+`payload_json`/+kernel `sequence`; arcand `translate_event` tool arms (64KB wire cap on results, drain-before-synthesize terminal fix); arcan-proxy `SubstrateEventTranslator` building real `life.v1.EventRecord`s — fixes Phase-1 TOKEN-text drop. New tool-lifecycle e2e (real tick → `fs.write` → frames at the proxy). P20 round-1 findings all fixed. |
| #1687 | `71c52f50` | **Ergon gap closure**: `WorkflowRunInputs` gains `stream_sink_factory` / `budget_gate` / `response_scorer` / `soul_attester`; runner composes `FanoutSink([Buffer, factory(session,branch)])`; arcan serve wires `LagoSink` (first `ergon-life-sinks` consumer, same journal as the kernel store — replay-visible), `EconomicBudgetGate` (Hibernate denies / Hustle clamps; port re-applies independently — hook adds visibility), `NousAdapter` (BRO-1225 implemented: evaluator fan-out, fail-open), `AgentAttestationAdapter` (BRO-1226 implemented: custody-JWS boundaries; serve wiring awaits custody config, noop warns at boot). `fully_wired` e2e proves sink + all three hooks fire. |
| #1688 | `866244b7` | **Deps majors**: rig-core 0.36 (4-site migration in arcan-provider) + rmcp 1.7 (11-site non-exhaustive migration in praxis-mcp-bridge, protocol tests green) + lance 0.28 (reverted by #1689 — see below). Supersedes dependabot #1043/#1040/#1042. |
| #1242 | `97f3f5d6` | **`life init` Anima bootstrap** (reviewed + merged): did:key P-256 identity + derived Base wallet via `InProcessAnima`, atomic 0o600 seed, schema-versioned `soul.json`, idempotent re-init. Spec H Sub-A. |
| #1243 | `b5428a8a` | **Spec H** (onboarding & capability UX roadmap, 15 sub-phases) merged. |
| #1034–#1038 | various | clap 4.6.1, indexmap 2.14, uuid 1.23.3, open 5.3.5, tracing-appender 0.2.5 (dependabot, rebased + merged). |
| deny.toml (in #1686) | `5b4d7bb1` | RUSTSEC-2026-0173 ignored (proc-macro-error2 unmaintained via tabled), stale RUSTSEC-2026-0002 dropped. |

**Closed-with-reason:** #1043/#1040/#1042 (superseded by #1688);
`@dependabot ignore this minor version` set for lance 0.28 on #1042.

## Lessons / process notes (cost me time — don't repeat)

1. **lance 0.28 cannot enter the tree yet**: it fails CI's Linux
   stable clippy *inside the dep* (`queries overflow the depth limit`,
   `lance-0.28.0/src/index.rs:543`); local macOS rustc 1.93 is fine.
   Re-try on a future lance release or toolchain bump; the
   LLVM-exception license allowance is already in deny.toml (dormant).
2. **`gh pr merge` does NOT enforce the non-required lanes**
   (Dependency Check, Lint, Test are not branch-protection-required).
   `gh pr checks --watch --fail-fast` exits 0 even on failure. Verify
   the full check roster explicitly before merging — the #1688 slip
   briefly broke two main lanes (repaired by #1689).
3. **Long-running implementation subagents must commit incrementally**
   — one died on the session usage limit with zero commits and the
   work was redone in-session (memory entry
   `subagent-checkpoint-discipline` saved).

## First action

```bash
gh pr checks 1689   # expect all green incl. Dependency Check + Lint
gh pr merge 1689 --squash --delete-branch
# then: 1041/1039 — merge when dependabot finishes re-rebase + CI green
gh pr list
```

## Pickup state (open threads ≤5)

- [ ] **Merge #1689** (main lane repair) + the last two dependabot
  bumps (#1041, #1039) when green → backlog zero.
- [ ] **Ops: put arcand in the prod chat path** — deploy arcand beside
  lifed (UDS sockets in lifegw-stack or TCP transport decision), flip
  `LIFED_ARCAN_BACKEND` to the real substrate; the tool-event wire
  (#1686) and browser decoding are already live. Then scopes:
  `tool:`/`fs:` + `APPROVAL_REQUIRED` UI (lifegw `auth/scope.rs`).
- [ ] **Anima custody config for arcan serve** — wire
  `AgentAttestationAdapter` at boot once a stable agent DID/custody
  config exists (`.life/identity/` from #1242 is the natural source —
  connect `life init` identity → arcan serve soul attestation).
- [ ] **`arcan agent test` live-LLM mode** (BRO-1008) — last
  architecture-audit gap.
- [ ] **Linear backfill** — MCP unauthenticated all session: link
  BRO-1225/1226 (implemented), BRO-1008, Spec H Sub-A done state, and
  the four PRs above when re-authed.

## Related context

- Prior handoffs: `2026-06-10-chat-agent-grounding-shipped.md` (Stage 1),
  `2026-06-09-chat-agent-grounding-and-harness.md` (Stage 0).
- Architecture-audit gap list: root `CLAUDE.md` §"Wired vs stubbed"
  (updated in #1687 — only the `arcan agent test` gap remains).
- arcan-ergon contract: `crates/arcan/arcan-ergon/CLAUDE.md`
  (spec-deviations 2+3 closed with the new wiring contract).
