# Cognitive Storage v2 — Revised Architecture After Research

> **Date**: 2026-04-01
> **Status**: Vision (revised after research synthesis)
> **Builds on**: COGNITIVE_STORAGE.md + external research on AI-native infrastructure

## Key Research Findings That Change the Design

1. **Memory IS the next scaling law** (MemOS, NeurIPS 2025) — capability jumps come from memory governance, not parameter scaling
2. **Agent performance = data movement problem** (Computer Architecture lens) — right information, wrong layer, wrong time
3. **Strategic forgetting > total recall** (Letta V1) — context pollution worse than information gaps
4. **GraphRAG: 90% hallucination reduction** — subgraph retrieval, not flat text passages
5. **ACID shared knowledge graph** (SurrealDB) — eliminates message passing for multi-agent coordination
6. **Deterministic replay** (Akka event sourcing) — debug by re-executing, not reading logs
7. **Composable retrieval at query time** (Qdrant) — agents shift strategies mid-workflow
8. **Delta Tensor** — embeddings + structured data + telemetry in one ACID table
9. **True self-improvement requires intrinsic metacognition** (ICML 2025) — not fixed human-designed loops
10. **80% of databases now created by agents** (Neon) — database IS agent infrastructure

## Revised Technology Stack

### Previous proposal:
```
Delta Lake → event storage
DuckDB     → analytics
Lance      → vectors
```

### Revised proposal:
```
Core Storage:
  Lance          → ALL persistent storage (events + vectors + metadata)
                   Local-first, embedded, zero-copy versioning
                   "SQLite for AI" — the right analogy
                   100x faster random access than Parquet

Knowledge Layer:
  Graph Index    → Causal event graph (edges over Lance records)
                   GraphRAG-style subgraph retrieval
                   Multi-hop reasoning over connected facts
                   Can use SurrealDB or custom index over Lance

Query Engine:
  DataFusion     → SQL over Lance (already integrated)
  DuckDB         → Analytical queries (reads Lance/Parquet natively)

Intelligence Layer:
  Context Compiler  → THE PRODUCT (assembles optimal LLM context)
  Consolidation     → Background memory governance
  EGRI Engine       → Self-evaluating improvement loops
```

### Why Lance as the unified store (not Delta Lake)

| Requirement | Delta Lake | Lance |
|------------|-----------|-------|
| Append-only events | ✅ | ✅ |
| Multi-writer | ✅ (optimistic) | ✅ (optimistic) |
| Time travel | ✅ | ✅ (version control) |
| Vector search | ❌ (need separate Lance) | ✅ (native ANN) |
| Embeddings storage | ❌ (Delta Tensor is new/unstable) | ✅ (native) |
| Random access | Slow (Parquet is columnar scan) | ✅ (100x faster) |
| Embedded/local-first | Needs object store abstractions | ✅ (filesystem native) |
| SQL queries | Via DataFusion | Via DataFusion (same!) |
| Rust native | ✅ | ✅ |
| Zero-copy versioning | ✅ | ✅ (Git-like) |

**Lance gives us everything Delta gives us PLUS native vector search and fast random access, in a single embedded format.** We don't need two storage engines — Lance unifies events + vectors.

## Revised Architecture

```
┌─────────────────────────────────────────────────────────┐
│                  Context Compiler                        │
│  "The memory controller for the LLM CPU"                │
│                                                          │
│  Given: task + token budget                              │
│  Produce: optimal context window                         │
│                                                          │
│  Strategies:                                             │
│  - Preserve agent's reasoning chain (full fidelity)      │
│  - Compress environment observations (aggressive)        │
│  - Strategic forgetting (drop low-relevance)             │
│  - Diversity sampling (MMR over retrieved items)         │
│  - GraphRAG (subgraph retrieval for multi-hop reasoning) │
│  - Self-evaluate (did the context actually help?)        │
└────────────────────┬────────────────────────────────────┘
                     │ queries
┌────────────────────┴────────────────────────────────────┐
│              Cognitive Memory Store                       │
│                                                          │
│  ┌─────────────────────────────────────────────────┐    │
│  │             Lance Dataset                        │    │
│  │  (unified storage: events + vectors + metadata)  │    │
│  │                                                   │    │
│  │  Tables:                                          │    │
│  │  - episodic    (conversations, tool calls)        │    │
│  │  - semantic    (facts, patterns, knowledge)       │    │
│  │  - procedural  (recipes, workflows, approaches)   │    │
│  │  - meta        (EGRI evaluations, self-metrics)   │    │
│  │  - causal_edges (decision → evidence → outcome)   │    │
│  │                                                   │    │
│  │  Every record has:                                │    │
│  │  - structured fields (session, type, timestamp)   │    │
│  │  - content (natural language)                     │    │
│  │  - embedding (semantic vector)                    │    │
│  │  - importance score (updated by access patterns)  │    │
│  │  - causal links (parent, children, evidence)      │    │
│  └─────────────────────────────────────────────────┘    │
│                                                          │
│  ┌─────────────────────────────────────────────────┐    │
│  │          Graph Index (over Lance)                 │    │
│  │  Causal chains: decision → evidence → outcome     │    │
│  │  Knowledge graph: concept → relation → concept    │    │
│  │  GraphRAG: subgraph retrieval for reasoning       │    │
│  │  Implementation: edge table in Lance + BFS/DFS    │    │
│  └─────────────────────────────────────────────────┘    │
│                                                          │
│  ┌─────────────────────────────────────────────────┐    │
│  │          Consolidation Engine                     │    │
│  │  Episodic → Semantic (pattern extraction)         │    │
│  │  Semantic → Procedural (approach testing)         │    │
│  │  Decay: reduce importance of unused memories      │    │
│  │  Strengthen: boost frequently-accessed memories   │    │
│  │  Strategic forgetting: prune low-value records    │    │
│  │  Runs: after each session + daily background      │    │
│  └─────────────────────────────────────────────────┘    │
│                                                          │
│  ┌─────────────────────────────────────────────────┐    │
│  │          EGRI Engine (self-improvement)            │    │
│  │  Evaluate: was the context useful? (utilization)  │    │
│  │  Govern: update retrieval strategy                │    │
│  │  Recurse: try modified approach                   │    │
│  │  Improve: commit better strategy to procedural    │    │
│  │  Intrinsic metacognition (not fixed loops)        │    │
│  └─────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────┘
                     │
┌────────────────────┴────────────────────────────────────┐
│              Analytics Layer (for humans)                 │
│  DuckDB: SQL over Lance tables                           │
│  DataFusion: programmatic queries                        │
│  Dashboards: cost, tool usage, memory growth             │
│  Deterministic replay: re-execute any session            │
└──────────────────────────────────────────────────────────┘
                     │
┌────────────────────┴────────────────────────────────────┐
│              Multi-Agent Coordination                     │
│  Shared Lance dataset (workspace.lance/)                 │
│  ACID writes via optimistic concurrency                  │
│  No message passing — agents read/write same graph       │
│  Real-time sync: Lance → Spaces (distributed)            │
└──────────────────────────────────────────────────────────┘
```

## The MemCube Concept (from MemOS)

Each memory unit in Lance is a **MemCube** — a container with:

```rust
struct MemCube {
    // Identity
    id: String,              // ULID
    version: u64,            // Lance version (auto-managed)

    // Content
    content: String,         // natural language
    embedding: Vec<f32>,     // semantic vector (384 or 768 dim)
    structured: Value,       // type-specific structured data

    // Cognition metadata
    tier: MemoryTier,        // episodic | semantic | procedural | meta
    kind: CognitionKind,     // perceive | deliberate | decide | act | verify | reflect
    importance: f32,         // 0-1, updated by access patterns
    confidence: f32,         // 0-1, how certain
    decay_rate: f32,         // how fast relevance fades

    // Causal links
    caused_by: Vec<String>,  // what led to this
    leads_to: Vec<String>,   // what this caused
    evidence_for: Vec<String>, // what decisions this supports

    // Lifecycle
    created_at: u64,
    last_accessed: u64,
    access_count: u32,
    session_id: String,

    // Operations: clone, merge, branch (Git for knowledge)
}
```

MemCubes can be:
- **Cloned** — copy a memory for a branch/experiment
- **Merged** — combine memories from multiple agents
- **Branched** — create a variant for A/B testing approaches
- **Superseded** — new fact replaces old (with version history)
- **Decayed** — importance reduced over time if not accessed
- **Strengthened** — importance increased on retrieval (synaptic potentiation)

## Implementation in Lago

### What changes:

```
lago-core         → Add MemCube type, MemoryTier enum, CognitionKind enum
lago-lance        → NEW: Lance-backed storage (replaces lago-delta proposal)
lago-graph        → NEW: Causal graph index over Lance records
lago-compiler     → NEW: Context Compiler (THE core product)
lago-consolidator → NEW: Background memory governance engine
lago-query        → KEEP: DuckDB analytics (reads Lance natively)
lago-journal      → KEEP: RedbJournal for backward compat
```

### What stays the same:

- `Journal` trait (adds `LanceJournal` implementation)
- `lago-store` (blob storage, unchanged)
- `lago-knowledge` (evolves to use Lance for vector search)
- `lago-api` (adds context compilation endpoint)
- `lago-fs` (filesystem manifests, unchanged)

## The Context Compiler — Product Spec

### Input
```rust
struct ContextRequest {
    task: String,           // what the agent is trying to do
    budget_tokens: usize,   // context window budget
    session_id: String,     // current session (for working memory)
    workspace: Path,        // for git/file context
    strategy: Strategy,     // balanced | recency | relevance | diversity
}
```

### Output
```rust
struct CompiledContext {
    sections: Vec<Section>,       // ordered by importance
    tokens_used: usize,
    relevance_score: f32,         // estimated quality
    compilation_trace: Trace,     // what was considered, what was dropped
}
```

### Algorithm
```
1. EMBED task → query vector

2. RETRIEVE from each tier (parallel):
   Episodic:   "similar past sessions" (ANN in Lance, top 20)
   Semantic:   "relevant facts"       (ANN + keyword, top 20)
   Procedural: "approaches that work" (ANN + success filter, top 10)
   Meta:       "self-improvement data" (latest EGRI scores)
   Graph:      "causal subgraphs"     (BFS from relevant nodes, depth 3)

3. SCORE each retrieved item:
   score = 0.4 * semantic_similarity
         + 0.2 * recency_decay(age)
         + 0.2 * importance
         + 0.1 * diversity_bonus(MMR)
         + 0.1 * success_rate(for procedural)

4. STRATEGIC FORGETTING:
   Drop items below threshold (score < 0.3)
   Compress long episodic items to summaries
   Preserve agent's own reasoning chain at full fidelity

5. PACK into budget:
   Greedy knapsack by score/token ratio
   Reserve: 5K tokens for system prompt
   Reserve: current task + tool schemas
   Fill remainder with highest-scored items

6. FORMAT for LLM:
   Natural language sections, not JSON
   "Last time you worked on similar code, you..."
   "A proven approach for this type of problem is..."
   "Recent workspace activity shows..."

7. SELF-EVALUATE (after LLM responds):
   Did the agent reference compiled context? → strengthen
   Did the agent search for something not in context? → weaken
   Update importance scores based on utilization
```

## Relationship to MemGPT/Letta

Our architecture is **MemGPT done right with better storage**:

| Concept | MemGPT/Letta | Lago Cognitive Storage |
|---------|-------------|----------------------|
| Working memory | Context window | Same |
| Archival storage | PostgreSQL rows | Lance (vector-native) |
| Memory management | LLM-driven tool calls | Context Compiler (systematic) |
| Strategic forgetting | Manual send_message | Automated consolidation |
| Self-improvement | None | EGRI engine |
| Multi-agent | Separate instances | Shared Lance dataset (ACID) |
| Graph reasoning | None | Causal graph + GraphRAG |
| Deterministic replay | None | Event-sourced from Lance |
| Time travel | None | Lance versioning |

## What Makes This Different From Everything Else

1. **Unified storage** — Lance replaces Delta + separate vector DB + separate graph DB
2. **Context Compiler as core product** — not an afterthought, THE interface
3. **Five-tier memory with consolidation** — biological memory model, not database model
4. **Causal event graph** — decisions linked to evidence and outcomes
5. **Strategic forgetting** — prune what hurts, keep what helps
6. **Self-evaluating retrieval** — the system improves its own context quality
7. **MemCube operations** — clone, merge, branch memories (Git for knowledge)
8. **Shared ACID knowledge graph** — multi-agent coordination without message passing
9. **Deterministic replay** — debug by re-executing, not reading logs
10. **Local-first embedded** — no server, no cloud dependency, filesystem-native
