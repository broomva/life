# Memory Graph Architecture

> **Date**: 2026-04-03
> **Status**: Phase 3 implemented (`BRO-446`)
> **Linear**: `BRO-444`, `BRO-445`, `BRO-446`, `BRO-447`
> **Scope**: Lago graph projection + Arcan retrieval tool over the cognitive substrate

## One-Sentence Summary

**`memory_graph` is a bounded, provenance-preserving retrieval layer built over canonical memory artifacts, not a second database.**

## Why This Exists

The current memory system can answer:

- keyword questions via `memory_search`
- recency questions via `memory_recent`
- semantic similarity questions via `memory_similar`

It cannot yet answer causal-chain questions well:

- What led to this decision?
- Which evidence supports this pattern?
- What outcome came from this prior fix?
- What related memories sit around this note or session summary?

Those are graph-shaped queries, not flat-search queries.

## Design Principles

### 1. Graph Is a Derived Index, Not Source of Truth

The source of truth remains:

- Lago journal events
- Lance memory records
- markdown memory artifacts in `.arcan/memory/*.md`
- `MEMORY.md`

The graph must be rebuildable from those artifacts.

### 2. Provenance Is Mandatory

Every returned node and edge must point back to a source artifact, event, or note.

### 3. Traversal Must Be Bounded

Graph retrieval is useful only if it stays small enough for prompt consumption. `memory_graph` must always enforce depth and node-count bounds.

### 4. Hybrid Retrieval Beats Topology Alone

Pure BFS is structurally correct but often low precision. The architecture should start with bounded graph traversal and then evolve toward hybrid graph + semantic ranking.

### 5. Degrade Gracefully

If graph projection is unavailable, the system still has:

- `memory_search`
- `memory_recent`
- `memory_similar`

`memory_graph` should fail clearly or fall back conservatively, never poison the prompt with unbounded output.

## Dependency Chain

### Architectural Dependencies

- `Lago` owns canonical persistence and derived graph projection
- `Arcan` owns the agent-facing retrieval tool surface
- `arcan-lago` is the adapter boundary between Arcan and Lago graph retrieval

### Repo / Package Dependencies

- `core/life/lago/crates/lago-knowledge`
  - wikilink extraction
  - note index
  - BFS traversal
- `core/life/lago/crates/lago-api`
  - existing traversal request/response semantics
- `core/life/arcan/crates/arcan`
  - memory tool implementations
  - shell tool registration
- `core/life/arcan/crates/arcan-lago`
  - retrieval adapter layer

### Runtime Dependencies

- markdown memory artifacts must exist and be indexable
- shared workspace memory should remain available through `workspace.lance`
- graph traversal must not require a separate graph daemon or service

### Data / Schema Dependencies

- memory notes with frontmatter
- wikilinks extracted by `lago-knowledge`
- later: structured causal metadata projected into edges

### API / Tool Dependencies

- `lago-api` already exposes `search` + `traverse`
- `memory_graph` should mirror those semantics at the Arcan tool boundary

### CI / Validation Dependencies

- unit tests in `lago-knowledge`
- integration tests in `arcan`
- shell E2E smoke for `memory_graph`

## Existing Reusable Components

### `lago-knowledge`

Already provides:

- `KnowledgeIndex::build`
- `resolve_wikilink`
- BFS traversal via `KnowledgeIndex::traverse`

Current traversal result:

- `path`
- `name`
- `depth`
- `links`

This is enough for a v1 graph retrieval path.

### `lago-api`

Already defines graph-shaped request/response semantics:

- `TraverseRequest { target, depth, max_notes }`
- `TraverseResponse { notes }`

This means the graph boundary is already conceptually stable.

## V1 Delivery Strategy

V1 should prefer reuse over new surface area.

### Preferred V1 Path

Implement `memory_graph` in Arcan over existing bounded traversal primitives.

That means:

- no new graph database
- no mandatory new Lago HTTP route
- no new persistence format

The first working implementation can be built from:

- `KnowledgeIndex::resolve_wikilink`
- `KnowledgeIndex::traverse`
- existing memory-tool patterns in Arcan

### Optional V1.5 Path

If an external client or symmetry requirement justifies it later, Lago can expose a first-class `/v1/memory/graph` route.

That should be treated as optional API hardening, not as a blocker for `memory_graph` itself.

### `arcan` Memory Tool Pattern

Current memory tools already establish:

- tool definitions
- read-only annotations
- JSON result shaping
- shell registration
- focused unit tests

`memory_graph` should follow this pattern directly.

## Ownership Split

### `lago-knowledge`

Owns:

- note resolution
- graph adjacency
- traversal primitives
- graph result primitives

Should not own:

- agent prompt formatting
- shell tool contracts

### `arcan-lago`

Owns:

- start-node resolution policy
- graph retrieval adapter
- hybrid ranking composition with Lance
- result shaping for Arcan

Should not own:

- shell registration
- direct user-facing prompt prose

### `arcan`

Owns:

- `memory_graph` tool definition
- shell registration
- final output formatting for the LLM
- E2E coverage

## Data Model

The first stable schema should be narrow.

### Node Types

- `memory`
- `decision`
- `evidence`
- `outcome`
- `pattern`
- `artifact`
- `session_summary`

### Edge Types

- `references` is the only shipped v1 edge type.
- `derived_from`, `supports`, `contradicts`, `led_to`, and `caused_by` are planned
  semantic edge types for a future typed-memory phase.

### Node Contract

```rust
pub struct MemoryGraphNode {
    pub node_id: String,
    pub node_type: String,
    pub title: String,
    pub summary: String,
    pub source_ref: String,
    pub depth: usize,
    pub outgoing_links: Vec<String>,
    pub score: f32,
    pub rank_signals: MemoryGraphRankSignals,
}

pub struct MemoryGraphRankSignals {
    pub depth: f32,
    pub query: f32,
    pub semantic: f32,
    pub importance: f32,
    pub recency: f32,
    pub edge_weight: f32,
}
```

`outgoing_links` is derived from the actual returned, capped edge set. It is not
the raw note link list. `score` and `rank_signals` are advisory ranking metadata
for prompt shaping and evaluation; provenance remains `source_ref`.

### Edge Contract

```rust
pub struct MemoryGraphEdge {
    pub source: String,
    pub target: String,
    pub edge_type: String,
    pub label: String,
    pub source_ref: String,
}
```

### Retrieval Result Contract

```rust
pub struct MemoryGraphResponse {
    pub found: bool,
    pub start: String,
    pub root: Option<String>,
    pub nodes: Vec<MemoryGraphNode>,
    pub edges: Vec<MemoryGraphEdge>,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub truncated: bool,
    pub depth: usize,
    pub max_nodes: usize,
    pub max_edges: usize,
    pub edge_filter: Vec<String>,
    pub query: Option<String>,
    pub ranking_backend: String,
}
```

The LLM-facing v1 tool output is structured JSON only. `truncated` is set from
explicit node/edge overflow detection, not from result size equaling the
configured limits.

## Start-Node Resolution

The tool must not require opaque internal IDs only.

Resolution order:

1. exact manifest path, for example `/notes/foo.md`
2. relative path, for example `notes/foo.md`
3. path stem, for example `notes/foo`
4. plain wikilink target, for example `Foo`
5. bracketed wikilink target, for example `[[Foo#heading]]`
6. non-error empty JSON from the Arcan tool boundary when no start node resolves

This keeps the tool ergonomic while staying deterministic by default.

## Tool Contract

### Input

```json
{
  "start": "auth-middleware-regression",
  "query": "why did auth middleware regress?",
  "depth": 2,
  "max_nodes": 12,
  "max_edges": 16,
  "edge_types": ["references"]
}
```

### Output Requirements

The tool should return:

- `root` as the resolved note path
- a bounded set of related nodes
- capped `references` edges
- provenance for every node and edge
- `found`, `truncated`, count, bound, and edge-filter metadata
- `query`, `ranking_backend`, `score`, and `rank_signals` when ranking is active

The tool should not return an unbounded adjacency dump.

When the start node is missing, `arcan::memory_tools::MemoryGraphTool` returns
the same top-level response shape with `found = false`, `root = null`, empty
`nodes`/`edges`, zero counts, and the bounded request metadata. This lets
clients recover without special-casing transport errors.

## Traversal Strategy

### V1

Use bounded BFS over `lago-knowledge` traversal primitives.

Defaults:

- `depth = 2`
- `max_nodes = 12`
- `max_edges = 16`

### V2

Hybrid ranking now preserves BFS as the no-query fallback and switches to
ranked candidate selection when `query` or semantic score hints are present.
The adapter overfetches bounded graph candidates, scores them, keeps the root
first, then truncates back to `max_nodes`.

Current signals:

- depth proximity
- lexical query relevance over title, summary, body, and tags
- optional semantic similarity from Arcan-provided Lance score hints
- frontmatter importance
- frontmatter recency
- graph edge weight inside the candidate set

Conceptually:

```text
score = depth
      + query_relevance
      + semantic_similarity
      + importance
      + recency
      + edge_weight
```

The exact coefficients can evolve after evaluation, but they live in
`arcan-lago` so prompt-facing behavior stays deterministic and testable.
Arcan owns the Lance call and passes normalized score hints into the adapter;
`arcan-lago` does not depend on Lance or embedding providers.

## Scope Model

The graph must preserve scope.

Required scopes:

- session-local
- workspace-shared
- user/global memory

Returned results must make scope visible. The agent should never confuse session-local causality with workspace-wide knowledge.

## Implementation Phases

### Phase 1 — `BRO-444`

Ship the contract:

- design doc
- node/edge schema
- ownership split
- traversal defaults
- validation plan

### Phase 2 — `BRO-445`

Ship v1:

- [x] bounded graph retrieval over `lago-knowledge`
- [x] `memory_graph` tool in Arcan shell
- [x] deterministic tests for chain, cycle, bounds, missing start, and edge filtering
- [x] no mandatory new Lago route

Implementation notes:

- `lago-knowledge::KnowledgeIndex::resolve_note_ref()` accepts exact manifest
  paths, relative paths, path stems, plain wikilink targets, and bracketed
  wikilinks.
- `arcan-lago::memory_graph` owns bounded response shaping with
  `MemoryGraphQuery`, `MemoryGraphResponse`, compact nodes, `references` edges,
  provenance paths, capped `outgoing_links`, explicit truncation flags, and hard
  caps.
- `arcan::memory_tools::MemoryGraphTool` maps missing start nodes to a clear
  schema-stable, non-error empty JSON result so agents can recover by trying
  `memory_search`, `memory_browse`, or `memory_similar`.

### Phase 3 — `BRO-446`

Ship hybrid ranking:

- [x] Lance-assisted node ranking via optional score hints
- [x] optional query-conditioned expansion
- [x] graph-only fallback

Implementation notes:

- `MemoryGraphQuery::query` activates ranked candidate selection without making
  embeddings mandatory.
- `MemoryGraphRankingHints` accepts optional semantic scores keyed by memory
  title/path/name. Arcan populates those hints from `workspace.lance` when an
  embedding provider is configured.
- `ranking_backend` is one of `graph_bfs`, `hybrid_lexical_graph`, or
  `hybrid_vector_graph`.
- Query-conditioned retrieval overfetches bounded candidates, promotes relevant
  non-root nodes above plain BFS neighbors, then re-applies `max_nodes` and
  `max_edges`.
- Missing embeddings, missing Lance datasets, or vector-search errors degrade
  to lexical graph ranking or BFS rather than failing the tool.

### Phase 4 — `BRO-447`

Ship validation and evaluation:

- shell E2E
- regression fixtures
- retrieval metrics
- provenance checks

## Testing Strategy

### Unit Tests

`lago-knowledge`:

- linear chain
- branching graph
- cycle handling
- missing start node
- bounded traversal

### Adapter Tests

`arcan-lago`:

- start-node resolution order
- edge filtering
- deterministic result ordering
- scope-preserving output
- lexical and semantic ranking promotion while preserving bounds

### Tool Tests

`arcan`:

- valid schema
- bounded result size
- readable summary formatting
- fallback behavior when graph unavailable
- query-conditioned ranking
- Lance-backed semantic hints when available

### E2E

- shell smoke can call `memory_graph`
- output remains bounded
- provenance survives formatting
- query-conditioned and graph-only paths both validated

## What We Are Explicitly Not Doing

- No separate graph database in v1
- No SurrealDB / Neo4j / graph service dependency
- No authoritative graph writes outside canonical memory writes
- No unbounded graph expansion into the prompt

## Acceptance Criteria

This design is complete when:

- the crate ownership split is unambiguous
- the graph is explicitly modeled as a derived index
- the tool contract is stable enough for implementation
- the ranking evolution path is defined
- the testing strategy is concrete

## Recommended First Code Slice

The first implementation slice after this doc should be:

1. add graph result types in `lago-knowledge` or `arcan-lago`
2. build a bounded adapter over existing `KnowledgeIndex::traverse`
3. expose `memory_graph` in Arcan
4. add unit tests plus shell smoke

That is the smallest production-ready slice that changes behavior without introducing architectural debt.
