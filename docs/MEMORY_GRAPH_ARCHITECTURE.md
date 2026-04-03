# Memory Graph Architecture

> **Date**: 2026-04-03
> **Status**: Design specification
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

- `references`
- `derived_from`
- `supports`
- `contradicts`
- `led_to`
- `caused_by`

### Node Contract

```rust
pub enum MemoryGraphNodeType {
    Memory,
    Decision,
    Evidence,
    Outcome,
    Pattern,
    Artifact,
    SessionSummary,
}

pub struct MemoryGraphNode {
    pub node_id: String,
    pub node_type: MemoryGraphNodeType,
    pub title: String,
    pub summary: String,
    pub source_ref: String,
    pub importance: Option<f32>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}
```

### Edge Contract

```rust
pub enum MemoryGraphEdgeType {
    References,
    DerivedFrom,
    Supports,
    Contradicts,
    LedTo,
    CausedBy,
}

pub struct MemoryGraphEdge {
    pub from: String,
    pub to: String,
    pub edge_type: MemoryGraphEdgeType,
    pub weight: Option<f32>,
    pub provenance: String,
}
```

### Retrieval Result Contract

```rust
pub struct MemoryGraphResult {
    pub root: MemoryGraphNode,
    pub nodes: Vec<MemoryGraphNode>,
    pub edges: Vec<MemoryGraphEdge>,
    pub summary: String,
}
```

The LLM-facing tool output should include compact prose plus the structured payload.

## Start-Node Resolution

The tool must not require opaque internal IDs only.

Resolution order:

1. direct graph node id
2. exact note path
3. exact note name
4. wikilink resolution
5. semantic narrowing if a query is present
6. fail with candidate suggestions

This keeps the tool ergonomic while staying deterministic by default.

## Tool Contract

### Input

```json
{
  "start": "auth-middleware-regression",
  "depth": 2,
  "limit": 12,
  "edge_types": ["caused_by", "supports", "led_to"],
  "query": "what led to the auth regression?"
}
```

### Output Requirements

The tool should return:

- one root node
- a bounded set of related nodes
- labeled edges
- a compact natural-language summary
- provenance for every node and edge

The tool should not return an unbounded adjacency dump.

## Traversal Strategy

### V1

Use bounded BFS over `lago-knowledge` traversal primitives.

Defaults:

- `depth = 2`
- `max_nodes = 12`
- `max_edges = 16`

### V2

Add hybrid ranking:

- depth penalty
- node importance
- recency
- semantic similarity
- edge weight

Conceptually:

```text
score = semantic_similarity
      + importance
      + recency_bonus
      + edge_weight
      - depth_penalty
```

The exact coefficients can evolve after evaluation.

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

- bounded graph retrieval over `lago-knowledge`
- `memory_graph` tool in Arcan
- deterministic tests
- no mandatory new Lago route

### Phase 3 — `BRO-446`

Ship hybrid ranking:

- Lance-assisted node ranking
- optional query-conditioned expansion
- graph-only fallback

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

### Tool Tests

`arcan`:

- valid schema
- bounded result size
- readable summary formatting
- fallback behavior when graph unavailable

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
