# Module Guide

All 76 crates organized by tier. Start at Tier 1 and work down.

## Tier 1 -- Start Here

Foundation crates with minimal dependencies. Read these first.

| Crate | Module | What It Does |
|-------|--------|-------------|
| `aios-protocol` | aiOS | Canonical types, traits, and event taxonomy for the entire OS |
| `praxis-core` | Praxis | Tool trait, sandbox policy, filesystem boundaries |
| `autonomic-core` | Autonomic | Homeostasis types: economic modes, hysteresis gates |

## Tier 2 -- Core

The main engines. Each implements a key OS primitive.

| Crate | Module | What It Does |
|-------|--------|-------------|
| `arcan-core` | Arcan | Agent loop types: Provider, Tool, Middleware, AgentEvent |
| `lago-core` | Lago | Journal trait, event envelope, session/branch IDs |
| `nous-core` | Nous | Evaluator trait, EvalScore, quality layers |
| `anima-core` | Anima | AgentSoul, AgentIdentity, AgentBelief, PolicyManifest |
| `haima-core` | Haima | Payment types, x402 protocol, task billing |
| `life-vigil` | Vigil | OpenTelemetry spans, GenAI semantic conventions, metrics |

## Tier 3 -- Full Systems

Complete modules with HTTP APIs, daemons, and bridges.

| Crate | Module | What It Does | Binary |
|-------|--------|-------------|--------|
| `arcan` | Arcan | Installable agent runtime (shell, serve, chat) | `arcan` |
| `arcand` | Arcan | Agent loop, SSE server, HTTP router (library) | -- |
| `lagod` | Lago | Persistence daemon (REST + gRPC) | `lagod` |
| `autonomicd` | Autonomic | Homeostasis daemon | `autonomicd` |
| `haimad` | Haima | Finance daemon | `haimad` |
| `nousd` | Nous | Evaluation daemon | `nousd` |
| `life-relayd` | Relay | Remote session relay daemon | `life-relayd` |
| `life-cli` | CLI | Deployment and onboarding CLI | `life-cli` |

## Tier 4 -- Advanced / Bridges

Crates that connect modules together. You only need these when integrating.

| Crate | Connects | Purpose |
|-------|----------|---------|
| `arcan-lago` | Arcan <> Lago | Event persistence, memory tools |
| `arcan-spaces` | Arcan <> Spaces | Distributed networking tools |
| `arcan-aios-adapters` | Arcan <> aiOS | Port adapters for kernel runtime |
| `autonomic-lago` | Autonomic <> Lago | Event-driven homeostatic projection |
| `haima-lago` | Haima <> Lago | Finance event persistence |
| `nous-lago` | Nous <> Lago | Evaluation event persistence |
| `anima-lago` | Anima <> Lago | Soul/belief persistence |
| `lago-aios-eventstore-adapter` | Lago <> aiOS | EventStorePort adapter |
| `praxis-mcp-bridge` | Praxis <> MCP | Model Context Protocol server/client |

## Internal Crates (Skip Unless Contributing)

These are implementation details. Consumers don't need them.

| Crate | Purpose |
|-------|---------|
| `aios-events` | Event type definitions |
| `aios-kernel` | Kernel trait (imported via aios-protocol) |
| `aios-policy` | Policy evaluation engine |
| `aios-runtime` | Kernel runtime implementation |
| `aios-sandbox` | Sandbox session management |
| `aios-tools` | Built-in tool definitions |
| `arcan-commands` | Shell slash-command registry |
| `arcan-console` | Web console assets |
| `arcan-fleet` | Multi-agent orchestration |
| `arcan-harness` | Tool harness (Praxis bridge) |
| `arcan-provider` | LLM provider implementations |
| `arcan-provider-bubblewrap` | bwrap sandbox provider |
| `arcan-provider-local` | Local subprocess provider |
| `arcan-provider-vercel` | Vercel AI sandbox provider |
| `arcan-sandbox` | Session sandbox store |
| `arcan-store` | Legacy JSONL store |
| `arcan-tui` | Terminal UI client |
| `arcan-anima` | Arcan <> Anima bridge |
| `arcan-opsis` | Arcan <> Opsis bridge |
| `arcan-praxis` | Arcan <> Praxis bridge |
| `lago-api` | Lago HTTP API routes |
| `lago-auth` | JWT authentication |
| `lago-billing` | Billing/tier logic |
| `lago-cli` | Lago CLI |
| `lago-compiler` | Query compiler |
| `lago-fs` | Filesystem manifest tracking |
| `lago-ingest` | Ingestion pipeline |
| `lago-journal` | redb journal implementation |
| `lago-knowledge` | Knowledge index (frontmatter, wikilinks, search) |
| `lago-lance` | Lance vector journal |
| `lago-policy` | RBAC policy engine |
| `lago-store` | Blob store (SHA-256 + zstd) |
| `autonomic-api` | Autonomic HTTP API |
| `autonomic-controller` | Pure rule engine |
| `haima-api` | Haima HTTP API |
| `haima-insurance` | Insurance primitives |
| `haima-outcome` | Outcome tracking |
| `haima-wallet` | Wallet (secp256k1, ChaCha20) |
| `haima-x402` | x402 protocol implementation |
| `nous-api` | Nous HTTP API |
| `nous-heuristics` | Inline evaluators (< 2ms) |
| `nous-judge` | LLM-as-judge evaluators |
| `nous-middleware` | Arcan middleware integration |
| `anima-identity` | Cryptographic identity (Ed25519, secp256k1, DID) |
| `praxis-tools` | Built-in tool implementations (fs, shell, edit) |
| `praxis-skills` | SKILL.md discovery and registry |
| `life-relay-api` | Relay local API server |
| `life-relay-core` | Relay wire protocol |
| `life-paths` | Shared path resolution |
| `opsis-core` | World state types |
| `opsis-engine` | Event processing engine |
| `opsis-lago` | Opsis <> Lago bridge |
| `opsisd` | Opsis daemon |
| `spaces-a2a` | A2A protocol bridge |
| Facade crates | `life-aios`, `life-arcan`, `life-lago`, `life-praxis`, `life-autonomic`, `life-haima`, `life-nous`, `life-anima`, `life-relay`, `life` -- thin re-export layers for downstream consumers |

## Dependency Graph

```
                        aios-protocol
                             |
        +--------+-----------+-----------+--------+--------+
        |        |           |           |        |        |
   arcan-core  lago-core  praxis-core  nous-core anima-core autonomic-core
        |        |           |           |        |        |
        v        v           v           v        v        v
      arcand   lagod    praxis-tools   nousd   anima-id  autonomicd
        |        |           |           |
        +---+----+----+------+-----------+
            |         |
          arcan    life-cli
       (binary)   (binary)
```

All modules depend on `aios-protocol` (the kernel contract). Modules never import each other's internals -- only bridge crates (e.g., `arcan-lago`, `nous-lago`) connect them.
