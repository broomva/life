# Research: Agent Framework Landscape Analysis (Q2 2026)

**Date**: 2026-04-02
**Subjects**: [Hermes Agent](https://github.com/nousresearch/hermes-agent) (Nous Research), [DeerFlow](https://github.com/bytedance/deer-flow) (ByteDance)
**Purpose**: Extract patterns and features to strengthen Life/AOS

---

## Executive Summary

Two of the most-starred open-source agent frameworks of early 2026 were analyzed:

| | Hermes Agent | DeerFlow |
|---|---|---|
| **Stars** | 21,816 | 56,180 |
| **Stack** | Python 3.11+, SQLite, Rich | Python + TS, LangGraph, FastAPI, Next.js |
| **Philosophy** | Self-improving personal agent | Super agent harness for research |
| **License** | MIT | MIT |
| **Release** | v0.6.0 (2026-03-30) | v2.0 (2026-02-28) |

Life/AOS differentiates on: event-sourced persistence, formal kernel contract, homeostatic regulation, Rust safety, and biological-analog architecture. But both projects offer patterns we should adopt.

---

## Key Learnings

### 1. Middleware Chain Architecture (from DeerFlow)

**What**: DeerFlow's lead agent passes every request through a 12-middleware chain in strict order: thread data, uploads, sandbox lifecycle, dangling tool call recovery, guardrails, summarization, todo list, title generation, memory, image viewing, subagent limits, clarification.

**Why it matters**: This is essentially the same pattern as HTTP middleware (tower/axum) applied to agent turns. It cleanly separates cross-cutting concerns from core agent logic.

**Recommendation for Life**: Arcan's 8-phase tick lifecycle already has structure, but formalizing a **middleware chain for the agent turn** would let us:
- Add guardrails as a middleware (pre-tool-call authorization)
- Add context summarization as a middleware (compress when near token limit)
- Add dangling tool call recovery as a middleware
- Make the pipeline extensible without touching the core loop

**Implementation target**: `arcan-core` — define a `TurnMiddleware` trait, compose a chain in `arcand`.

### 2. Progressive Skill Loading (from DeerFlow)

**What**: DeerFlow loads skill definitions lazily — only when the agent decides they're relevant. Skills are Markdown with YAML frontmatter (same as Praxis SKILL.md). 17 built-in skills include deep-research, chart-visualization, podcast-generation, etc.

**Why it matters**: Loading all skills into context wastes tokens. Progressive loading keeps context lean.

**Recommendation for Life**: Praxis already has SKILL.md discovery. Add:
- A **skill index** (name + one-line description) always available in context
- **On-demand full loading** — when the agent selects a skill, load the full SKILL.md
- Skill categories/tags for efficient retrieval

**Implementation target**: `praxis-skills` — add a `SkillIndex` that returns summaries, with `load_full(skill_name)` for detail.

### 3. Self-Improving Skills (from Hermes Agent)

**What**: After completing a complex task, Hermes automatically creates/updates reusable skill documents. The `skill_manage` tool lets the agent refine skills based on experience. Compatible with the agentskills.io open standard.

**Why it matters**: This is the missing link between "agent uses skills" and "agent gets better at using skills." Life's self-learning score is 2/10 — this directly addresses that gap.

**Recommendation for Life**: Extend Praxis skills with:
- A `skill_create` tool that writes new SKILL.md files from successful task patterns
- A `skill_refine` tool that updates existing skills with lessons learned
- Event-source skill mutations through Lago (every edit is an event, enabling replay/rollback)
- Consider compatibility with agentskills.io standard for ecosystem interop

**Implementation target**: `praxis-skills` + `arcan-lago` bridge for persistence.

### 4. Context Compression / Summarization (from both)

**What**: Both frameworks automatically compress conversation context when approaching token limits. DeerFlow uses a `SummarizationMiddleware` that summarizes completed sub-tasks. Hermes has a dedicated `context_compressor.py`.

**Why it matters**: Life currently has a context compiler with per-block budgets, but no automatic compression of completed work. Long agent sessions hit context limits.

**Recommendation for Life**: Add a summarization phase to the context compiler:
- After a tool execution completes, summarize the result if it exceeds a threshold
- Keep full detail for the most recent N turns, summarize older ones
- Store full history in Lago (event-sourced), serve compressed view to LLM

**Implementation target**: `arcan-core` context compiler — add a `CompressedBlock` variant.

### 5. Sub-Agent Delegation with Isolated Context (from both)

**What**: Both frameworks support spawning sub-agents with scoped context. DeerFlow uses a `task` tool with dual thread pools (3 scheduler + 3 execution workers, max 3 concurrent, 15-min timeout). Hermes uses `delegate_task` to spawn isolated subagents with their own iteration budget (50 vs 90 for parent).

**Why it matters**: Complex tasks decompose naturally into subtasks. Isolated contexts prevent cross-contamination and allow parallel execution.

**Recommendation for Life**: Design a sub-agent system:
- A `delegate_task` tool in Praxis that spawns a child Arcan session
- Child sessions inherit the parent's Lago journal but write to their own branch
- Lago's branching (already supported but unexposed) is the perfect substrate
- Autonomic's budget system provides natural iteration caps
- Results flow back as events to the parent session

**Implementation target**: `arcan-core` — `SubAgentExecutor`, leverage Lago branching + Autonomic budget.

### 6. Multi-Platform Gateway (from Hermes Agent)

**What**: Hermes serves 11+ messaging platforms (Telegram, Discord, Slack, WhatsApp, Signal, Matrix, etc.) from a single gateway process. Cross-platform conversation continuity is maintained.

**Why it matters**: Life has Spaces for agent-to-agent networking but no user-facing messaging integration. For real-world deployment, agents need to meet users where they are.

**Recommendation for Life**: This is a separate concern from the AOS kernel. Two approaches:
- **Short-term**: Build a thin gateway service that translates platform messages into Arcan chat requests and streams responses back. This could be a new crate `arcan-gateway`.
- **Long-term**: Spaces could evolve to be the universal messaging substrate with platform bridges.

**Implementation target**: New `arcan-gateway` crate (future phase).

### 7. Sandbox Isolation with Virtual Filesystem (from DeerFlow)

**What**: DeerFlow gives each task a full sandbox with virtual filesystem (`/mnt/user-data/{uploads,workspace,outputs}`, `/mnt/skills/`). Three providers: local, Docker, Kubernetes. Virtual path translation maps agent-visible paths to physical locations.

**Why it matters**: Life's security score is 4/10. Praxis has `FsPolicy` (workspace boundary enforcement) and `SandboxPolicy`, but no real OS-level isolation. No virtual filesystem mapping.

**Recommendation for Life**: Enhance Praxis sandbox:
- Add virtual path translation (agent sees `/workspace/`, mapped to real paths)
- Docker sandbox provider alongside the existing local provider
- Per-task workspace directories with automatic cleanup
- This aligns with the planned Aegis (security) project

**Implementation target**: `praxis-core` sandbox — add `SandboxProvider` trait with local/Docker impls.

### 8. Persistent Cross-Session Memory (from both)

**What**: Hermes uses file-based memory + SQLite FTS5 for full-text search across sessions. DeerFlow uses `MemoryMiddleware` that queues conversations for async memory extraction and deduplicates at apply time.

**Why it matters**: Life has MemoryProjection with 5 event types, but cross-session memory retrieval needs strengthening. The self-learning score of 2/10 reflects this.

**Recommendation for Life**: 
- Add FTS to `lago-knowledge` for searching across session events
- Async memory extraction after sessions (extract key facts, decisions, lessons)
- Memory deduplication at write time
- `lago-knowledge` already has scored search and graph traversal — wire it to the agent loop

**Implementation target**: `lago-knowledge` + `arcan-lago` bridge.

### 9. RL Training Pipeline (from Hermes Agent)

**What**: Hermes has built-in trajectory saving for RL training, batch runner for parallel processing, and integration with Nous Research's Atropos RL framework. Environments directory contains training setups for SWE-bench, web research, etc.

**Why it matters**: Self-improvement through RL is the frontier of agent capability. Life has no RL pipeline. This could dramatically boost the self-learning score.

**Recommendation for Life**: 
- Add trajectory serialization to Lago events (every agent turn is already persisted — add RL-compatible export)
- Design an evaluation harness that replays sessions and scores outcomes
- This is a Phase 2 (Self-learning) concern but should be designed now

**Implementation target**: `lago-core` — add trajectory export format. Future: eval harness.

### 10. Guardrails Middleware (from DeerFlow)

**What**: DeerFlow's `GuardrailMiddleware` runs before every tool call, providing pre-execution authorization checks. Separate from the tool-level permission system.

**Why it matters**: Life has Arcan's approval workflow (ApprovalGate, async pause/resume) but it's wired at the tool level. A middleware-level guardrail would catch dangerous patterns before they reach tools.

**Recommendation for Life**: Add guardrail checks as a turn middleware:
- Pattern matching on tool arguments (block dangerous commands, file paths)
- Rate limiting on expensive operations
- Budget checks via Autonomic before execution
- Compose with the existing ApprovalGate

**Implementation target**: `arcan-core` middleware chain.

---

## Patterns NOT to Adopt

| Pattern | Why Not |
|---|---|
| **Synchronous agent loop** (Hermes) | Life's async architecture is correct for a distributed system |
| **SQLite for persistence** (Hermes) | Lago's event-sourced redb journal is architecturally superior — append-only, replayable, content-addressed |
| **LangGraph/LangChain dependency** (DeerFlow) | Heavy framework lock-in. Life's kernel contract approach is more flexible |
| **Python monolith** (both) | Rust provides the safety, performance, and type guarantees Life needs |
| **OpenAI format as universal interface** (Hermes) | Life already has multi-format SSE adapters in Lago — keep provider abstraction at the protocol level |

---

## Priority Matrix

| Feature | Impact | Effort | Priority |
|---|---|---|---|
| Turn middleware chain | High | Medium | **P0** — architectural enabler |
| Context compression | High | Medium | **P0** — directly improves agent quality |
| Progressive skill loading | Medium | Low | **P1** — quick win |
| Self-improving skills | High | Medium | **P1** — addresses self-learning gap |
| Sub-agent delegation | High | High | **P1** — leverages existing Lago branching |
| Cross-session memory search | High | Medium | **P1** — lago-knowledge is ready |
| Guardrails middleware | Medium | Low | **P2** — compose with existing ApprovalGate |
| Sandbox isolation (Docker) | Medium | High | **P2** — aligns with Aegis roadmap |
| RL trajectory export | High | Medium | **P2** — Phase 2 dependency |
| Multi-platform gateway | Medium | High | **P3** — future phase |

---

## Recommended Implementation Order

1. **Turn middleware chain** in `arcan-core` (P0) — enables items 4, 5, 10
2. **Context compression middleware** (P0) — immediate quality improvement
3. **Progressive skill loading** in `praxis-skills` (P1) — low-effort, high-context-efficiency
4. **Self-improving skills** via Lago events (P1) — biggest self-learning unlock
5. **Cross-session FTS** in `lago-knowledge` (P1) — wire existing infra to agent loop
6. **Sub-agent delegation** via Lago branches (P1) — complex but high leverage

---

## References

- Hermes Agent: https://github.com/nousresearch/hermes-agent (v0.6.0, MIT)
- DeerFlow: https://github.com/bytedance/deer-flow (v2.0, MIT)
- agentskills.io: Open standard for agent skills (Hermes-compatible)
- Agent Communication Protocol (ACP): Used by DeerFlow for external agent invocation
