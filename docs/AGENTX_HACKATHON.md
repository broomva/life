# AgentX AgentBeats Hackathon — Research & Application Strategy

**Date**: 2026-03-03 | **Status**: Research Complete | **Phase 2 Active**: March 2 – May 3, 2026

## Overview

The [AgentX AgentBeats Competition](https://rdi.berkeley.edu/agentx-agentbeats) is Berkeley RDI's global agentic AI hackathon, open to a community of 40,000+ learners. Phase 2 (now active) has participants build **purple agents** (competitors) that are evaluated by **green agents** (benchmarks) using the Agentified Agent Assessment (AAA) paradigm.

**Prize pool**: $1M+. **Platform**: [agentbeats.dev](https://agentbeats.dev/). **Deadline**: May 3, 2026.

---

## Competition Structure

### Phase 2 Sprint Schedule

| Sprint | Dates | Tracks |
|--------|-------|--------|
| **1st** (ACTIVE NOW) | Mar 2 – Mar 22 | Game Agent, Finance Agent, **Business Process Agent** |
| **2nd** | Mar 23 – Apr 12 | **Research Agent**, Multi-agent Evaluation, τ²-Bench, **Computer Use & Web Agent** |
| **3rd** | Apr 13 – May 3 | **Agent Safety**, **Coding & Software Testing Agent**, Cybersecurity Agent |

### Key Protocol Requirements

Purple agents must:
1. **Implement the A2A protocol** (Agent-to-Agent) — the universal agent interoperability interface
2. **Support MCP** (Model Context Protocol) for tool access
3. **Register on agentbeats.dev** — public leaderboard, hosted evaluation
4. **No hardcoded answers** — must demonstrate genuine reasoning

---

## Project Alignment Analysis

### What We Have (Arcan + Lago Agent OS)

| Capability | Strength | Hackathon Relevance |
|------------|----------|---------------------|
| Full agent loop (reconstruct→call→execute→persist→stream) | 9/10 | All tracks |
| Event-sourced persistence (append-only, deterministic replay) | 10/10 | Evaluation trust |
| Multi-provider LLM support (Anthropic, OpenAI-compat, Mock) | High | Provider flexibility |
| 5-layer sandbox (Policy→Sandbox→FsPolicy→Tool→Audit) | High | Agent Safety track |
| Context compiler with per-block budgets | High | Research Agent track |
| Memory system (5 event types, MemoryProjection) | 8/10 | Long-horizon tasks |
| Approval workflow (ApprovalGate, async pause/resume) | High | Business Process track |
| RBAC policy engine (5 rules, 3 roles, 2 hooks) | High | Governance |
| 14 built-in tools (file I/O, shell, memory, MCP bridge) | High | Coding/Software Agent |
| Git-like branching on agent workspace | Unique | Speculative execution |
| 81 conformance tests + golden fixtures | High | Evaluation quality |
| Multi-format SSE streaming (OpenAI/Anthropic/Vercel/Lago) | High | Interoperability |

### What We Need to Build

| Gap | Priority | Effort |
|-----|----------|--------|
| **A2A protocol adapter** | CRITICAL — required to register | Medium (1-2 weeks) |
| **agentbeats.dev registration + hosted endpoint** | CRITICAL | Low (hours) |
| **HTTP endpoint exposing purple agent behavior** | High | Low (existing arcand) |
| Track-specific task handling | Medium | Varies per track |
| Observability/metrics reporting | Medium | Medium |

---

## Recommended Track Strategy

### Track 1: Business Process Agent (Sprint 1 — DEADLINE: March 22, 2026)

**Why we fit**: The Arcan approval workflow (M2.6 ApprovalGate) maps directly to business process automation. Event-sourced sessions provide full audit trails. RBAC policy engine governs what the agent can and cannot do. The async pause/resume pattern handles human-in-the-loop approvals — a defining feature of business process agents.

**Differentiators**:
- Deterministic replay = full process auditability
- ApprovalGate = native human escalation
- Policy engine = role-based access control per step
- Append-only journal = compliance-grade audit trail
- Branch/merge = parallel process exploration, rollback

**What to demo**: An agent that orchestrates a multi-step business workflow (e.g., document review → approval → commit) with full event-sourced traceability and policy-gated tool execution.

**Risk**: Sprint 1 ends March 22 — tight timeline. The A2A adapter must be built first.

---

### Track 2: Coding & Software Testing Agent (Sprint 3 — April 13–May 3)

**Why we fit**: Arcan was *built* for software engineering tasks. The harness (arcan-harness) provides sandboxed code execution, hashline-based edits (content-hash addressed, conflict-free), MCP tool bridge, and a full file I/O toolset. The context compiler manages code context budgets intelligently.

**Differentiators**:
- Hashline editing = deterministic, auditable code changes
- Sandbox (5-layer) = safe code execution
- Event-sourced session = full coding history, time-travel debug
- Branch/merge = speculative code exploration
- Golden fixtures = test-driven development posture

**What to demo**: An agent that takes a failing test, diagnoses the issue, writes a fix, runs tests in sandbox, and commits — all with a deterministic event trail.

**Risk**: Sprint 3 is latest; time to build is longer, but the fit is strongest here.

---

### Track 3: Agent Safety (Sprint 3 — April 13–May 3)

**Why we fit**: The 5-layer defense-in-depth sandbox is a first-class design principle. Policy-based capability gates (Allow/Deny/RequireApproval), filesystem path validation, environment whitelisting, and append-only audit trails are all built in. This could be positioned as a *safety-first* agent framework.

**Differentiators**:
- PolicyEngine with RBAC at every tool invocation
- SandboxPolicy: workspace boundary + env whitelist + timeout
- FsPolicy: path canonicalization prevents traversal attacks
- Every action in append-only journal = full forensic trail
- ApprovalGate: human must approve before privileged actions execute

**What to demo**: An agent that operates under a strict capability policy, escalates appropriately, and provides a complete forensic audit of all actions taken.

---

### Track 4: Research Agent (Sprint 2 — March 23–April 12)

**Why we fit**: Context compiler with typed blocks and per-block budgets is purpose-built for managing large context windows in research tasks. Memory projection system provides persistent, queryable knowledge across sessions. Multi-provider routing enables using different LLMs for different subtasks.

**Differentiators**:
- Context compiler: deterministic, budget-aware context assembly
- Memory system: persistent across sessions, governed tools
- Branching: explore multiple research hypotheses in parallel
- Event sourcing: full research provenance (every observation traceable)

---

## Implementation Plan

### Phase A: Foundation (Week 1 — March 3–10)

#### 1. Implement A2A Protocol Adapter

The A2A protocol is the critical gating requirement. Need to expose Arcan's agent loop via A2A-compliant endpoints.

**Key A2A concepts**:
- Agent Card (metadata about the agent, capabilities, endpoints)
- Task lifecycle (submitted → working → completed/failed)
- Message passing (text parts, tool results, artifacts)

**Implementation approach** (in `arcan` or new `arcan-a2a` crate):
```
arcan-a2a/
  src/
    server.rs     # A2A HTTP server (axum routes)
    agent_card.rs # Capability advertisement
    task.rs       # Task state machine
    bridge.rs     # Maps A2A messages ↔ AgentEvent
```

**A2A endpoints needed**:
```
GET  /.well-known/agent.json     # Agent Card
POST /                           # Send message (submit task)
GET  /{task_id}                  # Get task status
POST /{task_id}/cancel           # Cancel task
GET  /{task_id}/stream           # SSE stream (already have this!)
```

The SSE streaming infrastructure in `arcan-lago` already does exactly what the A2A stream endpoint needs — this is a thin adapter.

#### 2. Register on agentbeats.dev

- Create account
- Register purple agent with A2A endpoint URL
- Verify agent card is reachable

---

### Phase B: Sprint 1 Entry — Business Process Agent (March 10–22)

Build a demo showcasing the ApprovalGate-based business process workflow:

1. **Task**: "Review and approve the following expense report"
2. **Agent loop**: Parse task → validate policy → gate on approval → commit result
3. **Observable**: Every step in Lago journal, full SSE stream
4. **A2A wrapper**: Green agent submits task, purple agent responds with structured result

**Stretch**: Show branch/merge for parallel process paths (e.g., two reviewers independently, then merge decisions).

---

### Phase C: Sprint 2 Entry — Research Agent (March 23–April 12)

Leverage context compiler and memory for long-horizon research tasks:

1. Enable cross-session memory retrieval
2. Implement source citation in memory projections
3. Show multi-step research with branched hypothesis exploration

---

### Phase D: Sprint 3 Entry — Coding Agent + Safety (April 13–May 3)

This is our strongest fit. Use the full harness:

1. Accept coding task via A2A
2. Explore codebase with file tools
3. Write fix using hashline edits
4. Run tests in sandbox
5. Return structured result with full event trace

For safety track: demonstrate policy-gated execution with escalation trail.

---

## Technical Architecture for Competition

```
agentbeats.dev (Green Agent)
        │
        │  A2A Protocol (HTTP/JSON)
        ▼
  arcan-a2a (NEW — thin adapter)
        │
        │  Maps A2A ↔ AgentEvent
        ▼
  arcand (existing HTTP server)
        │
   Agent Loop
  ┌─────┴─────────────────────┐
  │  arcan-core               │
  │  arcan-harness (tools)    │
  │  arcan-provider (LLMs)    │
  └─────────┬─────────────────┘
            │ events
            ▼
       lago (persistence)
       redb journal + SSE stream
```

---

## Competitive Advantages

### Unique Technical Differentiators

1. **Rust + Compile-time safety**: No runtime panics from type errors; most agent frameworks are Python
2. **Event sourcing as first principle**: Every action is an immutable event — enables:
   - Deterministic replay (debugging, audit, compliance)
   - Time-travel debugging
   - Green agent can verify *everything* the purple agent did
3. **Defense-in-depth sandbox**: 5 layers of isolation — genuine safety, not theatrical
4. **Git-like branching**: Speculative execution paths, safe rollback — no other framework has this
5. **Zero external DB**: Embedded redb means the agent is fully self-contained
6. **Multi-format streaming**: Native OpenAI/Anthropic/Vercel/Lago compatibility

### Positioning Statement

> Arcan is a **production-grade Agent Operating System** built in Rust with event-sourced persistence, defense-in-depth sandboxing, and deterministic replay. Where other frameworks bolt safety on, Arcan builds it in from first principles — every action is an auditable event, every tool call is policy-gated, and every session is fully replayable.

---

## Risks & Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| A2A adapter takes too long | Medium | Start immediately; it's a thin HTTP wrapper over existing arcand |
| Sprint 1 deadline (Mar 22) too tight | Medium | Prioritize Business Process track first; fall back to Sprint 2+ |
| Observability gaps (2/10) hurt evaluation | Medium | Emit structured metrics alongside SSE events |
| Rust-based agent less familiar to evaluators | Low | A2A makes the protocol agnostic; evaluators see behavior, not code |
| Self-learning gap (0/10) | Low | Compensate with strong memory + context compiler |

---

## Next Steps (Immediate)

- [ ] **Register team on agentbeats.dev** (today)
- [ ] **Read A2A protocol spec** at docs.agentbeats.dev
- [ ] **Create `arcan-a2a` crate** — A2A adapter over arcand
- [ ] **Implement Agent Card** — advertise capabilities
- [ ] **Test with green agent** — run first evaluation
- [ ] **Submit for Business Process Agent track** (before March 22)
- [ ] **Deploy accessible endpoint** — agentbeats needs a public URL

---

## Resources

- Competition: https://rdi.berkeley.edu/agentx-agentbeats
- Platform: https://agentbeats.dev/
- Docs: https://docs.agentbeats.dev/
- A2A Protocol spec: https://docs.agentbeats.dev/ (look for protocol section)
- Registration form: link on competition page
