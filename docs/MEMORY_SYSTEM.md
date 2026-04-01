# Memory System — Integrating Claude Code Patterns with Cognitive Storage

> **Date**: 2026-04-01
> **Status**: Design spec
> **Integrates**: Claude Code memdir pattern + Cognitive Storage v2 + Filesystem Principle

## Core Insight

**Memory is a side-effect of compaction, not a separate system.**

When context pressure forces summarization, it simultaneously forces crystallization of knowledge. The same mechanism that frees context also produces durable memories.

## Memory Types (revised)

Combining Claude Code's practical four-type model with our cognitive tiers:

| Type | Claude Code | Cognitive Tier | What | Lifespan | Example |
|------|------------|---------------|------|----------|---------|
| `user` | ✅ user | — (new) | Who the user is — role, preferences, expertise | Long-lived, cross-session | "Senior Rust engineer, new to React" |
| `feedback` | ✅ feedback | Meta | Corrections and confirmations — learning signal | Permanent | "Don't mock the database in tests" |
| `project` | ✅ project | Semantic | Goals, deadlines, ongoing work | Project-scoped | "Merge freeze after Thursday" |
| `reference` | ✅ reference | Semantic | Pointers to external systems | Stable | "Bugs tracked in Linear INGEST project" |
| `episodic` | — | Episodic | What happened in past sessions | Decays | "Session 01KN: implemented hook system" |
| `procedural` | — | Procedural | Tested approaches that work | Success-weighted | "For auth bugs: check middleware order" |

The first four are Claude Code's proven model. The last two are our extensions for deeper cognitive memory.

## File Layout (Filesystem Principle)

```
.arcan/memory/
├── MEMORY.md              ← Master index (always in context, capped at 200 lines / 25KB)
│                            Each line: "- [Title](file.md) — one-line description"
│
├── user_role.md           ← User memory: role, preferences, expertise
├── user_preferences.md    ← User memory: communication style, tool preferences
│
├── feedback_testing.md    ← Feedback memory: "don't mock databases"
├── feedback_style.md      ← Feedback memory: "terse responses, no summaries"
│
├── project_goals.md       ← Project memory: current sprint goals
├── project_freeze.md      ← Project memory: merge freeze dates
│
├── reference_linear.md    ← Reference: Linear project links
├── reference_grafana.md   ← Reference: monitoring dashboards
│
├── episodic_session_01KN.md ← Episodic: session summary (auto-generated)
│
└── procedural_auth_bugs.md  ← Procedural: tested approach for auth bugs
```

Each memory file has YAML frontmatter:
```yaml
---
name: Testing approach for auth middleware
description: Don't mock the database — use integration tests against real DB
type: feedback
created: 2026-04-01
confidence: 0.95
---

Don't mock the database in auth middleware tests. We got burned last quarter
when mocked tests passed but the prod migration failed.

**Why:** Mock/prod divergence masked a broken migration.
**How to apply:** Always use testcontainers or a real test DB for auth tests.
```

## MEMORY.md — The Always-Loaded Index

```markdown
# Memory Index

## User
- [Role & Expertise](user_role.md) — Senior Rust engineer, agent OS developer

## Feedback
- [Testing approach](feedback_testing.md) — Don't mock databases, use integration tests
- [Response style](feedback_style.md) — Terse, no trailing summaries

## Project
- [Current goals](project_goals.md) — Arcan shell feature parity with Claude Code
- [Merge freeze](project_freeze.md) — Freeze after 2026-04-05 for release

## Reference
- [Linear projects](reference_linear.md) — Arcan Shell, Unified Runtime, Lago Lakehouse
```

**Rules:**
- Max 200 lines
- Max 25KB
- Always loaded into liquid prompt
- One line per memory, under ~150 chars
- Organized by type, not chronologically

## Compaction-Triggered Memory Extraction

When auto-compact fires (at ~100K tokens), SIMULTANEOUSLY:

```rust
fn auto_compact_with_memory(messages: &mut Vec<ChatMessage>, memory_dir: &Path) {
    // 1. Before compacting, scan conversation for memorable patterns
    let insights = extract_insights(messages);

    // 2. Deduplicate against existing memories
    let new_insights = deduplicate(insights, memory_dir);

    // 3. Write new memories to filesystem
    for insight in new_insights {
        write_memory_file(memory_dir, &insight);
        update_memory_index(memory_dir);
    }

    // 4. THEN compact the conversation
    compact_conversation(messages, COMPACT_TARGET);
}
```

### What to extract (by type):

**User memories** — detect when the user reveals:
- Their role: "I'm a data scientist", "I've been writing Go for ten years"
- Preferences: "always use Bun", "I prefer single PRs"
- Expertise: "first time touching React"

**Feedback memories** — detect when the user:
- Corrects: "no not that", "don't do X", "stop doing Y"
- Confirms: "yes exactly", "perfect, keep doing that"
- Explains why: "we got burned when..."

**Project memories** — detect when the user mentions:
- Goals: "we're building X by Thursday"
- Context: "the auth rewrite is driven by legal compliance"
- Deadlines: "freeze after Thursday"

**Reference memories** — detect when the user mentions:
- External systems: "bugs are tracked in Linear INGEST"
- URLs/dashboards: "grafana.internal/d/api-latency is the oncall board"
- People/teams: "talk to Sarah about the migration"

## Prompt Cache Optimization

Split the liquid prompt into cacheable and dynamic sections:

```rust
fn build_system_prompt(...) -> (String, String) {
    // CACHEABLE (stable across turns — gets prompt cache hits)
    let cacheable = vec![
        build_role_section(),           // "You are Arcan..."
        build_environment_section(),     // OS, shell, model
        load_project_instructions(),     // CLAUDE.md, AGENTS.md, rules
        build_guidelines_section(),      // behavioral rules
        // Tool schemas are injected by the API, also cached
    ].join("\n\n---\n\n");

    // DYNAMIC (changes per turn — always re-sent)
    let dynamic = vec![
        build_git_section(),            // branch, status (changes with commits)
        load_memory_index(),            // MEMORY.md (changes with new memories)
        build_skill_catalog(),          // active skill (changes with /skill)
        build_workspace_context(),      // recent agent activity (changes per turn)
    ].join("\n\n---\n\n");

    (cacheable, dynamic)
}
```

The Anthropic API caches the prefix of the system prompt. If our cacheable section stays identical across turns (which it will — CLAUDE.md doesn't change mid-session), we get **75% cheaper input tokens** on every turn after the first.

## Staleness Detection

Memories are claims about what was true at a given time. Before acting on a memory:

1. If the memory names a file path → verify the file exists
2. If the memory names a function → grep for it
3. If the memory describes repo state → check git log
4. If the memory conflicts with what's in context → trust current state, update memory

This is encoded in the system prompt guidelines:
```
When using information from memory:
- Verify file paths exist before referencing them
- Check that functions/methods still exist before recommending them
- If a memory conflicts with what you observe now, trust current state
- Update or remove stale memories rather than acting on outdated info
```

## Deduplication

Before creating a new memory, check existing ones:

```rust
fn deduplicate(new_insights: Vec<MemoryInsight>, memory_dir: &Path) -> Vec<MemoryInsight> {
    let existing = load_all_memories(memory_dir);
    new_insights.into_iter().filter(|insight| {
        !existing.iter().any(|existing| {
            // Same type + high content overlap = duplicate
            existing.memory_type == insight.memory_type
            && content_similarity(&existing.content, &insight.content) > 0.8
        })
    }).collect()
}
```

For now, content similarity can be keyword Jaccard overlap. With Lance, it becomes vector cosine similarity.

## Integration with Cognitive Storage

The memory system bridges Claude Code's file-based simplicity with our Cognitive Storage engine:

```
Filesystem (Level 0 — always works):
  .arcan/memory/*.md → readable, version-controlled, grep-able

Lago Engine (Level 2+ — enhances):
  MemCube records in Lance → semantic search, importance decay
  Context Compiler → optimal retrieval from both filesystem + Lance
  Consolidation → auto-organize memories over time
```

When the Lago engine is available:
- New memories are written to BOTH filesystem (.md) AND Lance (MemCube)
- Retrieval uses the Context Compiler (semantic search + keyword + tree navigation)
- Consolidation runs in background (merge similar memories, decay old ones)

When Lago is unavailable:
- Memories are filesystem-only (.md files)
- Retrieval is grep-based (keyword matching)
- No consolidation (manual via /memory command)

**The agent experience is identical.** The engine just makes it smarter.
