# Lago Architecture — The Cognitive Substrate for Agent OS

> **Date**: 2026-04-01
> **Version**: 3.0 (unified from COGNITIVE_STORAGE v1/v2, MEMORY_SYSTEM, LAKEHOUSE, FILESYSTEM_PRINCIPLE, SHARED_JOURNAL)
> **Status**: Canonical reference — supersedes all previous architecture docs

## One-Sentence Summary

**Lago is a cognitive substrate where the filesystem is the interface, Lance is the engine, the Context Compiler is the product, and the agent drives its own memory management.**

## Core Principles

### 1. The Filesystem Principle
The filesystem is the agent's interface. `.md` files, bash, and git always work. The lakehouse observes and enhances but never gates basic operation.

### 2. Agent-Driven Memory Management
The agent reasons about its own cognitive state. Retrieval strategies are tools the agent calls, not hardcoded rules. Context management is proactive, not reactive. Compaction is an emergency backstop, not the primary mechanism.

### 3. Memory as Side-Effect of Pressure
When context pressure forces summarization (compaction), it simultaneously forces crystallization of knowledge (memory extraction). The same mechanism that frees context produces durable memories.

### 4. Intrinsic Metacognition
The agent discovers and improves its own retrieval strategies over time. Fixed human-designed rules are starting points, not endpoints. EGRI tracks what works and feeds back into procedural memory.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Agent (LLM)                           │
│                                                          │
│  The agent REASONS about its own memory needs.           │
│  It calls retrieval tools proactively, not by rule.      │
│  It decides when to offload, retrieve, and forget.       │
│                                                          │
│  Available memory tools:                                 │
│  - memory_search(query)     → keyword scan               │
│  - memory_browse(tier,path) → tree navigation            │
│  - memory_similar(text)     → vector ANN search          │
│  - memory_graph(node_id)    → follow causal links        │
│  - memory_recent(n)         → recency-ordered recall     │
│  - memory_offload(content)  → save to episodic store     │
│  - memory_forget(id)        → mark as low-importance     │
└────────────────────┬────────────────────────────────────┘
                     │
┌────────────────────┴────────────────────────────────────┐
│              Context Compiler                            │
│                                                          │
│  Assembles the optimal context window on each turn.      │
│  Called by the agent's memory tools, not autonomously.    │
│                                                          │
│  Two modes:                                              │
│                                                          │
│  PROACTIVE (normal — agent-driven):                      │
│    Agent decides what context it needs                   │
│    Calls retrieval tools explicitly                      │
│    Manages its own working memory                        │
│    Offloads findings when done with a subtask            │
│                                                          │
│  REACTIVE (emergency — compaction backstop):             │
│    Context hits ~90% capacity                            │
│    Auto-compact fires (compress + extract)               │
│    Claude Code pattern: MicroCompact per-tool-result     │
│    Post-compact: restore key files + extract memories    │
│    Log as EGRI signal: "proactive management failed"     │
│                                                          │
│  Scoring (when retrieval tools are called):              │
│    semantic_similarity × recency × importance ×          │
│    diversity × tier_bonus × success_rate                 │
│                                                          │
│  Formatting:                                             │
│    Natural language, not JSON                            │
│    "In a previous session, you..."                       │
│    "A tested approach (95% confidence): ..."             │
└────────────────────┬────────────────────────────────────┘
                     │
┌────────────────────┴────────────────────────────────────┐
│              Cognitive Memory Store                       │
│                                                          │
│  ┌────────────────────────────────────────────────┐     │
│  │  Filesystem Layer (Level 0 — always works)     │     │
│  │                                                 │     │
│  │  MEMORY.md          → index (always in prompt)  │     │
│  │  memory/*.md        → individual memories       │     │
│  │  CLAUDE.md          → project rules             │     │
│  │  AGENTS.md          → operational boundaries    │     │
│  │  .life/control/policy.yaml → governance constraints  │     │
│  │  docs/*.md          → project knowledge         │     │
│  └────────────────────────────────────────────────┘     │
│                                                          │
│  ┌────────────────────────────────────────────────┐     │
│  │  Lance Layer (Level 2+ — enhances)             │     │
│  │                                                 │     │
│  │  workspace.lance/   → shared knowledge          │     │
│  │    ├── events       → all cognitive events       │     │
│  │    ├── memories     → MemCubes with embeddings   │     │
│  │    └── edges        → causal graph links         │     │
│  │                                                 │     │
│  │  sessions/<id>.redb → per-session conversation   │     │
│  │    (detail — tool calls, streaming deltas)       │     │
│  └────────────────────────────────────────────────┘     │
│                                                          │
│  ┌────────────────────────────────────────────────┐     │
│  │  Consolidation (Level 3 — background learning) │     │
│  │                                                 │     │
│  │  After session:                                 │     │
│  │    episodic → semantic (pattern extraction)      │     │
│  │    semantic → procedural (approach testing)      │     │
│  │    all tiers → decay unused, strengthen used     │     │
│  │                                                 │     │
│  │  After compaction (reactive):                   │     │
│  │    Extract user/feedback/project/reference       │     │
│  │    Deduplicate against existing memories         │     │
│  │    Update MEMORY.md index                        │     │
│  └────────────────────────────────────────────────┘     │
│                                                          │
│  ┌────────────────────────────────────────────────┐     │
│  │  EGRI Engine (Level 3 — self-improvement)      │     │
│  │                                                 │     │
│  │  Track: which retrieval strategy led to better  │     │
│  │  outcomes for which task types?                  │     │
│  │                                                 │     │
│  │  Feed back: successful strategies become         │     │
│  │  procedural memories                             │     │
│  │                                                 │     │
│  │  Signal: compaction events = proactive           │     │
│  │  management failure → improve strategy           │     │
│  └────────────────────────────────────────────────┘     │
└──────────────────────────────────────────────────────────┘
```

## Memory Types

Six types spanning practical (Claude Code) and cognitive (research-informed):

| Type | Source | What | Lifespan | Trigger |
|------|--------|------|----------|---------|
| `user` | Claude Code | Who the user is — role, preferences, expertise | Cross-session | User reveals identity/preferences |
| `feedback` | Claude Code | Corrections and confirmations — learning signal | Permanent | User corrects or confirms |
| `project` | Claude Code | Goals, deadlines, ongoing work | Project-scoped | User mentions project context |
| `reference` | Claude Code | Pointers to external systems | Stable | User mentions external resources |
| `episodic` | Cognitive research | What happened in past sessions | Decays with importance | Auto-extracted on session end |
| `procedural` | Cognitive research | Tested approaches that work | Success-weighted | Extracted from repeated patterns |

## The MemCube

Every memory unit — whether in a `.md` file or in Lance — is a MemCube:

```rust
struct MemCube {
    id: String,
    tier: MemoryTier,           // working|episodic|semantic|procedural|meta
    kind: CognitionKind,        // perceive|deliberate|decide|act|verify|reflect|consolidate|govern
    content: String,            // natural language
    embedding: Vec<f32>,        // semantic vector (optional)
    structured: Value,          // type-specific data

    // Causal graph
    caused_by: Vec<String>,
    leads_to: Vec<String>,
    evidence_for: Vec<String>,

    // Lifecycle
    importance: f32,            // updated by access patterns (synaptic potentiation)
    confidence: f32,            // how certain
    decay_rate: f32,            // how fast relevance fades
    access_count: u32,          // retrieval frequency
    created_at: u64,
    last_accessed: u64,
    session_id: String,
}
```

On filesystem, a MemCube is a `.md` file with YAML frontmatter. In Lance, it's a row with vector index.

## Context Management: Proactive vs Reactive

### Proactive (agent-driven, normal operation)

The agent is aware of its own context utilization and manages it deliberately:

```
Turn 1: User asks to fix auth bug
  Agent thinks: "I should check if I've solved similar bugs before"
  → Calls memory_search("auth bug fix") — finds procedural memory
  → Calls memory_graph(auth_decision_id) — follows causal chain
  → Now has relevant context WITHOUT filling entire window

Turn 5: Agent has been debugging for a while, context at 50%
  Agent thinks: "My debugging findings so far are valuable but taking space"
  → Calls memory_offload("Root cause: middleware ordering issue in...")
  → Frees working memory, preserves discovery in episodic store

Turn 10: Agent is about to start a new subtask
  Agent thinks: "The auth work is done, I need config context now"
  → Working memory naturally focuses on config
  → Auth findings are in episodic store, retrievable if needed
  → Context stays clean without ever hitting compaction
```

### Reactive (compaction backstop, emergency)

When proactive management fails — the context hits ~90% capacity:

```
Context at 93% → AUTO-COMPACT fires:

  1. MicroCompact: compress individual tool outputs
     (Bash, FileRead, Grep — shrink verbose outputs in-place)

  2. Macro-Compact: summarize old conversation turns
     (keep last 10 turns verbatim, summarize earlier ones)

  3. SIMULTANEOUSLY extract memories:
     - Scan compressed conversation for patterns
     - User corrections → feedback memory
     - Project context → project memory
     - Discoveries → episodic memory
     - Deduplicate against existing memories
     - Update MEMORY.md index

  4. Post-compact restore:
     - Reload up to 5 recently-read files (5K tokens each)
     - Reload MEMORY.md (may have new entries)

  5. EGRI signal:
     - Log: "compaction triggered at turn N, context was 93%"
     - Meta-memory: "agent didn't offload proactively for this task type"
     - Next similar task: agent will be more aggressive about offloading
```

## Prompt Architecture

### Cacheable Section (stable across turns — gets Anthropic prompt cache hits)

```markdown
# System
You are an AI coding assistant powered by Arcan, the Life Agent OS runtime...

# Environment
- Working directory: /Users/broomva/project
- Platform: darwin (aarch64)
- Date: 2026-04-01
- Model: claude-sonnet-4-5

# Project Instructions
{contents of CLAUDE.md + AGENTS.md + .claude/rules/*.md}

# Guidelines
- Read files before editing...
```

### Dynamic Section (changes per turn — always re-sent)

```markdown
# Git Context
- Branch: main
- Status: M src/lib.rs, ?? new_file.rs

# Memory Index
{contents of MEMORY.md — capped at 200 lines / 25KB}

# Active Skill
{skill-specific instructions if a skill is activated}

# Workspace Activity
{recent events from shared Lance journal}
```

### Prompt Cache Boundary

The cacheable section is sent first. If it's identical to the previous turn (which it will be — CLAUDE.md doesn't change mid-session), Anthropic caches it at **75% discount** on input tokens. The dynamic section follows and is always processed fresh.

## Crate Architecture

```
lago-core           → types: MemCube, EventEnvelope, Journal trait, MemoryTier, CognitionKind
lago-lance           → storage: LanceJournal (events + vectors + metadata in Lance format)
lago-compiler        → intelligence: Context Compiler (scoring, retrieval, formatting)
lago-journal         → storage: RedbJournal (per-session, backward compat)
lago-store           → storage: content-addressed blobs (SHA-256 + zstd)
lago-knowledge       → index: frontmatter parsing, wikilinks, scored search, BFS traversal
lago-consolidator    → PLANNED: background memory governance (episodic→semantic→procedural)
lago-query           → PLANNED: DuckDB analytical queries over Lance/Delta
lago-api             → HTTP: REST + SSE + memory endpoints
lago-fs              → filesystem: manifests, branching, diffs
lago-policy          → governance: RBAC, tool rules
lago-auth            → security: JWT middleware
```

## Retrieval Strategy as Tools (not rules)

The agent selects retrieval strategy by reasoning, not by hardcoded rules:

| Tool | What it does | When agent would use it |
|------|-------------|----------------------|
| `memory_search(query)` | Keyword scan across all memories | Quick lookup: "do I know about X?" |
| `memory_browse(tier, path)` | Navigate memory tree (PageIndex-style) | Exploring: "what approaches do I have for bug fixing?" |
| `memory_similar(text)` | Vector ANN search in Lance | Complex query: "find similar past experiences" |
| `memory_graph(node_id)` | Follow causal links | Debugging: "what led to this decision?" |
| `memory_recent(n)` | Last N memories by recency | Continuity: "what did I just do?" |
| `memory_offload(content)` | Save to episodic store + free working memory | Proactive management: "save this finding, move on" |
| `memory_forget(id)` | Reduce importance score | Cleanup: "this is no longer relevant" |

The agent learns which tools work best for which task types. That learning itself becomes procedural memory.

## Implementation Status

| Component | Status | Tests | Notes |
|-----------|--------|-------|-------|
| MemCube types | ✅ Complete | 11 | lago-core/cognitive.rs |
| EventEnvelope | ✅ Complete | 50+ | lago-core/event.rs |
| Journal trait | ✅ Complete | 6 | lago-core/journal.rs |
| RedbJournal | ✅ Complete | 20+ | lago-journal |
| LanceJournal | ✅ Complete | 24 | lago-lance (events + sessions + workspace) |
| Context Compiler | ✅ Complete | 27 | lago-compiler (scorer, formatter, retriever) |
| FilesystemRetriever | ✅ Complete | In compiler tests | Level 0 retrieval |
| Liquid Prompt | ✅ Complete | 13 | arcan-core/prompt.rs (cacheable/dynamic split) |
| Shared workspace journal | ✅ Complete | — | arcan shell → workspace.lance |
| Memory extraction | ✅ Complete | — | Heuristic extraction + compaction-triggered |
| MEMORY.md index | ✅ Complete | — | Always-loaded, capped 200 lines, auto-generated |
| Prompt cache boundary | ✅ Complete | — | SystemPrompt { cacheable, dynamic } split |
| Proactive memory tools | ✅ Complete | 6 tools | memory_search, memory_browse, memory_recent, memory_offload, memory_forget |
| /search command | ✅ Complete | 6 | Keyword search across memory files (2026-04-02) |
| Session resume | ✅ Complete | — | Replay from redb journal, tokio fallback fixed |
| Consolidation engine | ✅ Partial | 6 | Decay + prune implemented; pattern extraction basic |
| E2E test suite | ✅ Complete | 45 | 6-level smoke test, path-filtered CI |
| Vector search integration | 🔄 In Progress | — | BRO-382: Lance embedding column + ANN search |
| Embedding provider | 📋 Planned | — | BRO-388: HttpEmbeddingProvider, embed-on-write |
| memory_similar tool | 📋 Planned | — | BRO-390: Agent-driven vector retrieval |
| Tree navigation | 📋 Planned | — | PageIndex-style hierarchical retrieval |
| EGRI self-improvement | 📋 Planned | — | Track retrieval strategy outcomes |
| DuckDB analytics | 📋 Planned | — | Cross-session SQL queries |

## Graceful Degradation

| Level | What works | What's enhanced |
|-------|-----------|----------------|
| **0: Filesystem** | CLAUDE.md, memory/*.md, bash, git | Nothing — pure filesystem |
| **1: Events** | + RedbJournal session persistence | Session replay, deterministic debug |
| **2: Semantic** | + LanceJournal + vector search | Relevance-based retrieval |
| **3: Intelligence** | + Consolidation + EGRI | Auto-learning, pattern extraction |
| **4: Platform** | + Cloud storage + multi-tenant | Analytics, billing, compliance |

Each level is additive. Lower levels always work.
