# Cognitive Storage — AI-Native Infrastructure for Agent Cognition

> **Date**: 2026-04-01
> **Status**: Vision (architectural north star)
> **Context**: Critique of the Lago Lakehouse proposal — Delta + DuckDB + Lance are storage engines, but the real product is the Context Compiler.

## The Core Insight

**Traditional databases optimize for human queries. AI-native storage must optimize for LLM context windows.**

The fundamental constraint of an AI agent isn't storage capacity — it's the **200K token context window**. Everything in the infrastructure should answer one question: **"Given this task, what's the best possible context to send to the LLM?"**

This means the storage layer's primary output isn't query results — it's **compiled context**.

## Critique of Current Approach

### What we proposed
```
Delta Lake  → event storage (append-only, time travel)
DuckDB      → analytical queries (SQL over events)
Lance       → vector search (semantic similarity)
```

### Why it's insufficient

1. **We're copying Databricks, not reimagining for AI.** Delta/DuckDB/Lance were designed for data engineers and ML training pipelines. Agents have fundamentally different workload patterns: streaming-first, context-window-constrained, self-referential, causally-structured.

2. **Flat events lose cognitive structure.** An EventEnvelope has: id, session, seq, payload. But cognition is hierarchical: decisions have rationale, actions have outcomes, patterns have confidence levels. A flat event stream loses the causal graph.

3. **SQL is the wrong query language for agents.** An agent doesn't ask "SELECT COUNT(*) FROM events WHERE tool_name = 'bash'". It asks "what approach worked last time I faced a similar problem?" This requires semantic reasoning, not relational algebra.

4. **Memory is treated like data, not like cognition.** Current memory: write facts to files, load into context. Real memory has types (episodic, semantic, procedural, meta) with different retention, retrieval, and consolidation strategies.

5. **No self-improvement feedback loop.** The storage is passive — it stores what you write and returns what you query. AI-native storage should actively consolidate patterns, decay irrelevant memories, strengthen useful ones, and evaluate its own retrieval quality.

## The Cognitive Memory Model

### Five Memory Tiers

```
┌─────────────────────────────────────────────────────┐
│ Tier 1: Working Memory (context window)              │
│                                                       │
│ What: Current task, recent messages, active tools     │
│ Capacity: ~200K tokens                                │
│ Duration: Single turn / session                       │
│ Retrieval: Direct (it's already in context)           │
│ Storage: In-memory (ChatMessage array)                │
├─────────────────────────────────────────────────────┤
│ Tier 2: Episodic Memory (what happened)               │
│                                                       │
│ What: Full conversations, tool calls, outcomes        │
│ Capacity: Unbounded (append-only)                     │
│ Duration: Permanent (with decay scoring)              │
│ Retrieval: "Last time I did X" → replay               │
│ Storage: Delta Lake (time-ordered events)             │
├─────────────────────────────────────────────────────┤
│ Tier 3: Semantic Memory (what I know)                 │
│                                                       │
│ What: Facts, patterns, code understanding, rules      │
│ Capacity: Grows via consolidation                     │
│ Duration: Permanent (confidence-weighted)             │
│ Retrieval: "What do I know about X" → synthesis       │
│ Storage: Lance (vector-indexed knowledge)             │
├─────────────────────────────────────────────────────┤
│ Tier 4: Procedural Memory (how to do things)          │
│                                                       │
│ What: Workflows, tool sequences, tested approaches    │
│ Capacity: Grows via EGRI evaluation                   │
│ Duration: Permanent (success-rate-weighted)            │
│ Retrieval: "How should I approach X" → recipe          │
│ Storage: Lance (task-type-indexed procedures)         │
├─────────────────────────────────────────────────────┤
│ Tier 5: Meta-Memory (memory about memory)             │
│                                                       │
│ What: Self-improvement trajectories, EGRI trials      │
│ Capacity: Small, high-signal                          │
│ Duration: Permanent                                   │
│ Retrieval: "Am I getting better at X?" → trend         │
│ Storage: Delta Lake (evaluation records)              │
└─────────────────────────────────────────────────────┘
```

### Memory Consolidation (Background Process)

```
Episodic → Semantic (pattern extraction)
  "I've used this bash pattern in 5 sessions" → semantic fact

Semantic → Procedural (approach testing)
  "This approach worked 4/5 times for auth bugs" → procedure

Procedural → Rules (invariant extraction)
  "Always read before editing" → governance rule

All tiers → Meta (self-evaluation)
  "My retrieval quality improved 15% this week" → meta insight
```

Consolidation runs as a background process (like biological sleep):
- After each session: extract key patterns from episodic memory
- Daily: evaluate procedural memory success rates
- Weekly: update meta-memory with self-improvement trajectory

## The Context Compiler

**This is the core innovation — not the storage engines, but the intelligence that assembles context.**

```rust
/// The Context Compiler assembles the optimal context window for a given task.
///
/// It draws from all five memory tiers, scores items by relevance,
/// and packs them into the token budget ordered by expected utility.
trait ContextCompiler {
    /// Compile the best possible context for the current task.
    fn compile(&self, request: ContextRequest) -> CompiledContext;
}

struct ContextRequest {
    /// What the user/agent is trying to do
    task: String,
    /// Maximum tokens to fill
    budget_tokens: usize,
    /// Current workspace state (git, files, project)
    workspace: WorkspaceState,
    /// Which memory tiers to draw from (all by default)
    tiers: Vec<MemoryTier>,
    /// Preferences (e.g., prefer recent over relevant)
    strategy: CompilationStrategy,
}

struct CompiledContext {
    /// The assembled context, ready for LLM consumption
    sections: Vec<ContextSection>,
    /// Total tokens used
    tokens_used: usize,
    /// Estimated relevance quality (0-1)
    relevance_score: f32,
    /// What was included and what was dropped (for self-evaluation)
    compilation_log: CompilationLog,
}

struct ContextSection {
    /// Section name (e.g., "Relevant Past Experience", "Project Rules")
    name: String,
    /// Natural language content (NOT JSON, NOT SQL results)
    content: String,
    /// Source tier
    tier: MemoryTier,
    /// Relevance score for this section
    relevance: f32,
    /// Token count
    tokens: usize,
}
```

### How the Context Compiler Works

```
1. UNDERSTAND TASK
   Embed the current task → query vector

2. RETRIEVE FROM EACH TIER
   Episodic: "similar past conversations" (semantic search in Delta)
   Semantic: "relevant facts and patterns" (vector search in Lance)
   Procedural: "approaches that worked" (task-type match in Lance)
   Meta: "self-improvement insights" (latest EGRI evaluations)
   Workspace: git status, CLAUDE.md, AGENTS.md (filesystem)

3. SCORE AND RANK
   Each retrieved item scored by:
   - Semantic similarity to current task (cosine distance)
   - Recency (exponential decay)
   - Importance (pre-computed, updated by access patterns)
   - Diversity (MMR — avoid redundant context)
   - Success history (for procedural items)

4. PACK INTO BUDGET
   Greedy knapsack: highest scored items first
   Until token budget is filled
   Always reserve space for: system prompt, current task, tool schemas

5. FORMAT FOR LLM
   Natural language, not JSON
   Markdown sections with clear headers
   Narrative flow, not key-value dumps
   "Last time you worked on a similar problem, you..."

6. SELF-EVALUATE
   After the LLM responds, evaluate:
   - Did the context help? (did the agent use it?)
   - Was anything missing? (did the agent need to search for more?)
   - Was anything irrelevant? (did the agent ignore it?)
   Update memory importance scores based on utilization
```

## The Cognitive Event

The fundamental unit isn't an "event" — it's a **cognitive event** with causal structure:

```rust
struct CognitiveEvent {
    // Identity
    id: EventId,
    session_id: SessionId,
    timestamp: u64,

    // Cognition type (maps to aiOS 8-phase lifecycle)
    kind: CognitionKind,
    // Perceive    → what I observed
    // Deliberate  → what I considered
    // Decide      → what I chose and why
    // Act         → what I did
    // Verify      → did it work?
    // Reflect     → what did I learn?
    // Consolidate → pattern extracted
    // Govern      → rule updated

    // Causal graph edges
    caused_by: Vec<EventId>,     // what led to this event
    leads_to: Vec<EventId>,      // what this event caused
    evidence_for: Vec<EventId>,  // what decisions this supports

    // Content
    content: String,              // natural language description
    structured: Value,            // type-specific structured data

    // Embedding (computed asynchronously)
    embedding: Option<Vec<f32>>,

    // Memory metadata
    importance: f32,              // how significant (0-1, updated over time)
    confidence: f32,              // how certain (0-1)
    access_count: u32,            // retrieval frequency (strengthening)
    decay_rate: f32,              // how fast relevance fades
}
```

This maps directly to the **aiOS 8-phase tick lifecycle** — each phase produces cognitive events that link to each other causally.

## Revised Technology Stack

```
Context Compiler (THE PRODUCT)
    │
    ├── Cognitive Event Store
    │   ├── Episodic tier    → Delta Lake (append-only, time travel)
    │   ├── Semantic tier    → Lance (vector-indexed knowledge)
    │   ├── Procedural tier  → Lance (task-type-indexed recipes)
    │   ├── Meta tier        → Delta Lake (EGRI evaluations)
    │   └── Causal Graph     → Edge index over Delta events
    │
    ├── Consolidation Engine
    │   ├── Pattern extractor   (episodic → semantic)
    │   ├── Approach evaluator  (semantic → procedural)
    │   ├── Rule crystallizer   (procedural → governance)
    │   └── EGRI feedback loop  (meta → all tiers)
    │
    ├── Query Engine (for humans)
    │   └── DuckDB over Delta tables
    │       (dashboards, analytics, cost tracking)
    │
    └── Storage Backends
        ├── Delta Lake (delta-rs)  — events + causal edges
        ├── Lance                  — vectors + ANN search
        ├── YAML/Markdown          — rules, policies, identity
        └── Blob Store             — files, artifacts (existing)
```

## What Makes This Different

1. **Context-window-first design** — storage exists to serve the context compiler
2. **Tiered memory with consolidation** — not just storage, but learning
3. **Causal event graph** — decisions linked to evidence and outcomes
4. **Self-evaluating retrieval** — the system improves its own context quality
5. **LLM-native output** — natural language context, not query results
6. **EGRI-native** — self-improvement is a first-class storage operation

This isn't "Databricks for agents." This is **a cognitive substrate** — infrastructure that makes agents smarter over time by design.

## Relationship to Existing Architecture

```
aiOS (kernel contract)
  ├── 8-phase tick lifecycle → CognitionKind variants
  ├── AgentStateVector → working memory state
  └── EventKind taxonomy → CognitiveEvent mapping

Arcan (runtime)
  ├── Provider calls → Perceive/Act events
  ├── Tool execution → Act/Verify events
  ├── Nous evaluation → Reflect events
  └── Autonomic gating → Govern events

Lago (storage) — EVOLVES TO:
  ├── Delta Lake → episodic + meta tiers
  ├── Lance → semantic + procedural tiers
  ├── Context Compiler → THE NEW CORE
  └── Consolidation Engine → background learning

Praxis (tools)
  ├── Tool results → episodic events
  └── Skill catalog → procedural memory seed

Autonomic (regulation)
  ├── Budget tracking → meta-memory
  └── Economic modes → governance rules
```

## Open Questions

1. **Should consolidation use the LLM itself?** Pattern extraction from episodic to semantic memory could use an LLM call — but that costs money. Trade-off: quality vs cost.

2. **How to handle the cold start?** First session has empty memory. Should we pre-seed from CLAUDE.md/AGENTS.md? From the codebase structure? From git history?

3. **Multi-agent memory sharing.** When Agent A discovers a pattern, should it immediately become available to Agent B? Or go through a review/approval gate?

4. **Memory conflict resolution.** If two agents consolidate contradictory patterns, which wins? Confidence scoring? Recency? Human tiebreak?

5. **Privacy boundaries.** Some memories should be private to a user. Some should be shared across a team. How do tiers interact with access control?
