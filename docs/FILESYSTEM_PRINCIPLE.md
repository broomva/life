# The Filesystem Principle

> **The filesystem is the interface. Lago is the engine. The lakehouse provides intelligence underneath.**

## Core Principle

Agents interact with the world through files and bash. This is correct and must never change.

```
CLAUDE.md       → rules (human-readable, version-controlled)
AGENTS.md       → boundaries (framework-agnostic)
.arcan/memory/  → memories (readable .md files)
docs/           → knowledge (navigable documentation)
bash + git      → everything else
```

**No agent should ever need to know about Lance, embeddings, vector search, or the lakehouse to function.** The filesystem mode (Level 0) must always work.

## Graceful Degradation

| Level | Infrastructure | What the agent gets |
|-------|---------------|-------------------|
| 0 | Filesystem only | CLAUDE.md, memory/*.md, bash, git. Works. |
| 1 | + Event capture | Session replay, deterministic debug |
| 2 | + Semantic search | Better context via relevance retrieval |
| 3 | + Consolidation | Automatic pattern extraction, self-improvement |
| 4 | + Platform | Cloud, multi-tenant, analytics, billing |

Each level is additive. Lower levels always work. Higher levels make agents smarter.

## What This Means for Implementation

1. **Every memory must be a readable file.** MemCubes in Lance are the engine, but `.arcan/memory/*.md` files are the interface. The lakehouse writes both.

2. **Every config must be a readable file.** `.life/control/policy.yaml`, `CLAUDE.md`, `AGENTS.md` — never locked in a database.

3. **Git is the version control.** Lance has versioning, but git is what agents use. The lakehouse observes git, not replaces it.

4. **Bash is the escape hatch.** If the lakehouse is down, `cat`, `grep`, `find` still work.

5. **The Context Compiler reads from both.** Filesystem for rules and recent memory. Lakehouse for semantic search and historical patterns.

## The Lago Value Add

What the lakehouse provides that the filesystem can't:

- **Semantic search**: "find memories about auth patterns" → vector ANN
- **Cross-session intelligence**: patterns from 100+ sessions
- **Multi-agent coordination**: ACID writes to shared knowledge
- **Self-improvement**: EGRI evaluation over structured data
- **Analytics**: cost, tool usage, error rates
- **Deterministic replay**: re-execute any session
- **Consolidation**: episodic → semantic → procedural → rules

**None of these are required. All of them make agents better.**
