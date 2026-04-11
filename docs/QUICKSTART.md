# Quickstart

Get a running agent in 30 seconds, then decide where to go deeper.

## 30-Second Start

```bash
cargo install arcan
arcan shell --provider mock
```

**What just happened?** You started Arcan, the agent runtime, with a mock LLM provider that requires no API key. The shell is an interactive REPL where you can chat, execute tools, and explore the agent loop.

To use a real LLM, set an API key:

```bash
ANTHROPIC_API_KEY=sk-ant-... arcan shell
```

## What Do You Want to Do?

| Goal | Start Here | Key Crate |
|------|-----------|-----------|
| Build agents | `crates/arcan/` | arcan-core |
| Add tools | `crates/praxis/` | praxis-core |
| Persist state | `crates/lago/` | lago-core |
| Agent economics | `crates/haima/` | haima-core |
| Multi-agent networking | `crates/spaces/` | life-spaces |
| Evaluate quality | `crates/nous/` | nous-core |
| Regulate behavior | `crates/autonomic/` | autonomic-core |
| Agent identity | `crates/anima/` | anima-core |
| Observability | `crates/vigil/` | life-vigil |

## Recommended Learning Path

```
1. praxis-core     -- how tools work (sandbox, filesystem, editing)
2. arcan-core      -- how the agent loop runs (provider, events, state)
3. lago-core       -- how events persist (journal, blobs, sessions)
4. full stack      -- wire everything: arcan shell / arcan serve
```

## Next Steps

- **[MODULE_GUIDE.md](MODULE_GUIDE.md)** -- all 76 crates categorized by tier, with dependency graph
- **[ARCHITECTURE.md](ARCHITECTURE.md)** -- system design and data flow
- **[examples/](../examples/)** -- runnable code examples
- **[ROADMAP.md](ROADMAP.md)** -- where the project is headed
