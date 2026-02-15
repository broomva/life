# Arcan + Lago: a Rust-native agent OS with event-sourced persistence

**Arcan and Lago together represent the first Rust-native, event-sourced agent runtime stack designed to treat AI agent infrastructure as an operating system rather than a library.** In a market dominated by Python-first frameworks with ephemeral state, no sandboxing guarantees, and fragmented persistence, this pairing addresses fundamental reliability gaps that block production-grade autonomous agents. Arcan provides the runtime daemon—orchestrator loop, typed streaming events, and harness-quality sandbox guarantees— while Lago provides the persistence substrate: an append-only event journal, content-addressed blob storage, Git-like filesystem branching, and a policy engine for tool governance. Together, they form an “Agent OS” where every state transition is immutable, every action is replayable, and every tool invocation passes through compile-time-verified safety gates.

The AI agent platform market reached **~$7.8 billion in 2025**  and is projected to exceed **$50 billion by 2030** at a ~45% CAGR.  Yet every major framework in this space—LangChain, CrewAI, AutoGen, OpenAI Agents SDK—shares the same architectural DNA: Python runtimes, mutable state, trust-based execution, and bolt-on persistence. Arcan + Lago breaks from this pattern entirely.

-----

## The agent framework landscape has a reliability crisis

The 2024–2026 AI agent framework explosion produced over a dozen competing systems, all converging on similar architectural compromises. **LangChain/LangGraph**  (~95K GitHub stars) pioneered graph-based agent workflows with explicit state machines and checkpointing, but its heavy abstraction layers and steep learning curve have drawn persistent criticism.  **CrewAI** (~44K stars) simplified multi-agent coordination through role-based crews  and claims 5.76× faster execution than LangGraph,  but its logging infrastructure is notoriously broken—normal print and log functions fail inside Tasks, making production debugging painful. **Microsoft’s AutoGen** (~35K stars) championed conversational multi-agent patterns  before entering maintenance mode in October 2025, merging with **Semantic Kernel** into the Microsoft Agent Framework, creating migration uncertainty for existing users. 

**OpenAI’s Agents SDK** (March 2025) took a minimalist approach with four primitives—Agents, Handoffs, Guardrails, Sessions—supporting 100+ LLMs,  but lacks native graph workflows and parallel multi-agent execution. **Anthropic’s MCP** (Model Context Protocol) emerged as the connectivity standard   with **97 million monthly SDK downloads**   and 5,800+ servers, but security researchers identified prompt injection vulnerabilities and tool permission gaps within months of launch.  **Vercel AI SDK** (20M+ monthly downloads) dominates frontend-focused AI streaming  but isn’t designed for complex backend agent orchestration. **Haystack** and **LlamaIndex** (~18K and ~47K stars respectively) evolved from RAG-first frameworks into agent-capable systems, but their agent abstractions remain secondary to their data pipeline strengths.

Across this entire landscape, a survey of practitioners found **62% identified security as the top challenge** in deploying AI agents.  No mainstream framework treats sandboxing as a first-class primitive. State management approaches are fragmented and incompatible across frameworks. And not a single major agent framework uses event sourcing as its core persistence model—the very pattern that distributed systems have relied on for decades to guarantee auditability, reproducibility, and resilience.

## Agent runtime infrastructure still treats state as an afterthought

The infrastructure layer beneath agent frameworks reveals the same gaps. **E2B** provides Firecracker microVM sandboxes  with ~150ms startup times and hardware-level isolation (used by 88% of Fortune 100),  but offers no built-in event sourcing or state reconstruction—sandboxes run up to 24 hours   and then disappear. **Modal** ($1.1B valuation) delivers serverless GPU compute  with gVisor isolation and a distributed filesystem called Volumes,  but is Python-centric and SaaS-only with no self-hosting option. **Fly.io’s Sprites** (October 2025) introduced persistent VMs for AI coding agents with instant checkpoint/restore,  representing the closest production analogy to stateful agent infrastructure, but without deterministic state reconstruction from event logs. **Daytona** pivoted to agent runtime infrastructure in February 2025  with sub-90ms cold starts   and Docker-based sandboxing (upgradeable to Kata Containers),  but default isolation is the weakest in the market. 

**Replit Agent** offers the most sophisticated production agent system, with a multi-agent architecture (Manager, Editor, Verifier agents),  checkpoint-based state snapshots, and durable execution via the Mastra framework and Inngest. It achieves 90% autonomy success rates  and can run 200+ minutes autonomously.  But its checkpointing is snapshot-based, not event-sourced—you can revert to a checkpoint but cannot replay the sequence of events that produced it, losing the causal chain that enables true debugging and reproducibility.

The industry faces what practitioners call the **security-performance-simplicity trilemma**: microVMs offer strong isolation but add overhead; gVisor provides moderate isolation at lower cost but with syscall compatibility issues; Docker containers are fast but fundamentally insecure for untrusted agent code.  No runtime solves all three simultaneously. A Rust-native runtime changes this calculus: Rust’s memory safety guarantees reduce the attack surface within the runtime itself, potentially enabling lighter isolation techniques with equivalent security properties.

## Arcan’s architecture: the orchestrator, harness, and typed event system

Arcan is structured as a **seven-crate Rust workspace** with strict separation of concerns. The dependency graph flows from the thin CLI binary (`arcan`) through the daemon logic (`arcand`) down to foundational crates for protocol definitions, sandbox isolation, persistence, provider abstraction, and the Lago bridge.

### The orchestrator loop

The core agent loop lives in `arcan-core` and executes in `arcand`. It follows a disciplined cycle: receive user input and create a session event → send context to the LLM provider with streaming → parse tool call responses → execute tool calls within the harness sandbox → record all events to the append-only store → send tool results back to the LLM → iterate until the model produces a final response or the configurable `--max-iterations` safety limit is reached. Every transition in this loop produces a **typed streaming event**—a Rust enum/struct serialized as JSON and pushed to clients via Server-Sent Events. The type system guarantees at compile time that event schemas are consistent across the entire pipeline, eliminating the class of runtime serialization errors that plague Python agent frameworks.

The `arcand` daemon (following Unix naming conventions like `httpd` and `sshd`) exposes an HTTP API for session management, commands, and configuration, alongside the SSE endpoint for real-time event streaming. The CLI binary (`cargo install arcan`) is a thin wrapper that wires `arcand` components together with argument parsing: `arcan --port 3000 --data-dir .arcan --max-iterations 10`. 

### The harness: sandbox-first agent execution

`arcan-harness` implements what the project calls “harness quality”— a term borrowed from safety-critical systems where the test harness itself must meet the same reliability standards as the system under test. The harness provides three layers of protection:

**Filesystem guardrails** restrict which paths and operations agents can access. Unlike trust-based systems where agents have implicit access to the entire filesystem (the default in LangChain, CrewAI, and most frameworks), Arcan’s harness operates on an explicit allowlist model. Every file operation must pass through the guardrail layer before execution.

**Sandbox isolation** confines agent-executed code within a restricted environment. The process-level isolation leverages Rust’s ownership model to prevent resource leaks and ensures that a misbehaving tool cannot corrupt the runtime state.

**Hashline edit primitives**  represent a particularly clever design choice. Rather than specifying file edits by line number (which breaks when files change between the LLM reading the file and proposing an edit), hashline edits identify code sections by content hash. This makes edits robust against concurrent modifications and off-by-one errors—a persistent problem in AI coding agents where the model’s view of a file may be stale by the time the edit executes. Content-addressed line identification means edits are **idempotent and conflict-free** by construction.

### LLM provider abstraction

`arcan-provider` implements concrete LLM connections against trait-based interfaces defined in `arcan-core`. The current implementation targets **Anthropic Claude**,  with a built-in mock provider that allows the full agent loop to execute without API keys—critical for testing and development. The trait-based design means adding providers (OpenAI, Google, open-source models) requires implementing the provider trait without touching the orchestrator or harness code.

## Lago’s event-sourced persistence: the journal, store, and filesystem

Lago provides the durable substrate that makes Arcan’s agents long-lived and reproducible. Its **nine-crate architecture** separates concerns across the event journal, blob storage, filesystem operations, ingestion, API surface, policy enforcement, CLI tooling, and the daemon process.

### The append-only event journal

`lago-journal` implements the foundational event sourcing primitive: an **append-only, immutable event log** where all state is derived from the ordered sequence of events. No event is ever mutated or deleted. Current state is always a projection—a fold over the event stream from the beginning (or from a snapshot) to the present. This design, proven in distributed systems (Apache Kafka, EventStoreDB, Akka Persistence), provides three guarantees that no other agent framework offers natively:

- **Complete audit trail**: Every LLM call, tool invocation, file edit, and state transition is recorded as an immutable event with timestamps and causal ordering 
- **Deterministic state reconstruction**: Given the same event sequence, the same state is always produced—enabling **time-travel debugging** where developers can reconstruct the exact agent state at any point in its history 
- **Replay without live LLM calls**: Events can be replayed against modified agent logic to test changes without incurring API costs or non-deterministic LLM behavior

The event sourcing literature, validated by Akka and Confluent for AI agent systems, identifies this pattern as naturally fitting agentic workloads because communication with LLMs is inherently event-based and streaming, agents naturally separate read and write models (CQRS), and non-deterministic AI systems especially benefit from knowing not just *what* state an agent is in but *why*. 

### Content-addressed blob storage

`lago-store` implements a **content-addressed storage system using SHA-256 hashing with zstd compression**. Every blob (file content, model response, tool output) is stored by its content hash rather than by name or path. This yields several architectural advantages: automatic deduplication (identical content is stored once regardless of how many events reference it), integrity verification (any bit flip is detected by hash mismatch), and efficient diffing (changed content produces a new hash, unchanged content references the existing blob).

The choice of **zstd compression** is deliberate—it offers a superior compression ratio to gzip at comparable speeds, and the Rust `zstd` crate provides zero-copy decompression, minimizing memory allocations in the hot path.

### Git-like filesystem branching

`lago-fs` implements **filesystem branching and diffing** modeled on Git’s approach but optimized for agent workspaces. Agents can create branches of their working directory, explore different solution paths, and merge or discard branches based on outcomes. This enables capabilities that no existing agent framework provides:

**Speculative execution**: An agent can branch its workspace, attempt a risky operation (refactoring a codebase, deploying a configuration change), and either commit the branch on success or discard it on failure—without ever corrupting the main workspace state.

**Parallel exploration**: Multiple agent instances (or sub-agents) can work on different branches simultaneously, exploring alternative approaches to a problem. The branching model provides natural isolation between parallel workstreams while enabling eventual merge of successful results.

**Diffing and comparison**: Because branches track content-addressed blobs, computing the diff between two workspace states is efficient and precise. This powers observability tools that show exactly what changed between any two points in an agent’s execution history.

### Dual-protocol streaming: gRPC + HTTP/SSE

Lago exposes two complementary ingestion and streaming protocols. `lago-ingest` provides **bidirectional gRPC streaming via tonic** (the dominant Rust gRPC framework), optimized for high-throughput, low-latency internal communication between Arcan and Lago or between distributed Lago instances. `lago-api` provides an **Axum-based HTTP REST API with Server-Sent Events**, designed for client-facing consumption and compatibility with existing AI tooling ecosystems.

The SSE implementation supports **multi-format compatibility**: events can be streamed in formats compatible with **OpenAI’s streaming protocol**, **Anthropic’s event format**, and the **Vercel AI SDK’s streaming specification**. This means frontends built against any of these three major ecosystems can consume Lago’s event stream without adaptation—directly addressing the provider lock-in problem that plagues current agent deployments.

### The policy engine: RBAC for autonomous agents

`lago-policy` implements a **rule-based policy engine with Role-Based Access Control** for governing what tools agents can invoke and under what conditions. This is architecturally distinct from the guardrail systems in OpenAI’s Agents SDK (input/output validation) or MCP’s permission model (which security researchers have shown to be vulnerable to prompt injection and privilege escalation). 

Lago’s policy engine operates at the persistence layer, meaning **every tool invocation passes through policy evaluation before the event is committed to the journal**. Denied actions are recorded as policy violation events (maintaining the audit trail) but never execute. This creates an enforcement boundary that cannot be bypassed by prompt injection or agent misbehavior—the policy engine sits below the agent’s reasoning layer and above the execution layer.

### Embedded storage with redb

The entire persistence stack runs on **redb**, a pure-Rust embedded key-value store with ACID transaction guarantees. This choice eliminates external database dependencies entirely—no PostgreSQL, no Redis, no SQLite. The Lago daemon is a single binary with zero external runtime dependencies. redb provides crash-safe, concurrent read access, and B-tree-based storage with copy-on-write semantics that align naturally with Lago’s append-only event model. The “zero external deps” property means Lago can be deployed anywhere Rust compiles: bare metal, containers, edge devices, even WebAssembly targets.

## The arcan-lago bridge: composing runtime and persistence

The `arcan-lago` crate serves as the **integration bridge** between Arcan’s runtime and Lago’s persistence layer.  This bridge translates Arcan’s internal typed event model into Lago’s journaling format, ensuring that every event produced by the orchestrator loop—user messages, LLM responses, tool invocations, harness decisions, state transitions—flows into Lago’s append-only journal with full fidelity.

The bridge architecture means Arcan and Lago can evolve independently. Arcan can add new event types, providers, or harness capabilities without modifying Lago’s storage layer. Lago can optimize its journal compaction, blob deduplication, or policy rules without affecting Arcan’s orchestration logic. The bridge crate owns the mapping between these two domains—a clean separation that enables the “Agent OS” composability vision where different runtimes could potentially plug into Lago’s persistence, or Arcan could target different persistence backends.

## Six pain points this architecture eliminates

### State loss and ephemeral conversations

Every mainstream agent framework treats conversations as ephemeral by default. LangChain stores chat history in memory. CrewAI manages state through task outputs  that vanish between runs. Even frameworks with persistence (LangGraph’s checkpointing, Replit’s snapshots) store state as point-in-time snapshots rather than causal event sequences. When an agent session crashes, the context is lost. With Lago’s append-only journal, **no state is ever lost**—sessions can be resumed from any point by replaying the event stream.

### Non-reproducible agent behavior

AI agents are non-deterministic by nature (LLM responses vary with temperature, tool outputs change over time). But the orchestration layer should be deterministic: given the same events, the same state transitions should occur. Event sourcing makes this guarantee concrete. Developers can **replay a production incident** by feeding the exact event sequence into a local instance, reproduce the bug, fix the orchestrator logic, and verify the fix by replaying again—all without making a single LLM API call.

### Uncontrolled tool execution

The OWASP Top 10 for Agentic Applications (2026) identifies prompt injection, tool misuse, and lateral movement as top threats.  Most frameworks delegate sandboxing entirely to the developer. Arcan’s harness provides **defense-in-depth**: filesystem guardrails restrict path access, the sandbox confines execution, hashline edits prevent file corruption,  and Lago’s policy engine enforces RBAC on tool invocations at the persistence boundary. Every layer is enforced by Rust’s type system at compile time.

### Provider lock-in

Despite claims of model-agnosticism, most frameworks work best with specific providers: OpenAI’s Agents SDK optimizes for OpenAI,  Semantic Kernel integrates deepest with Azure,  Claude Code requires Anthropic models. Arcan’s trait-based provider abstraction combined with Lago’s multi-format SSE compatibility (OpenAI, Anthropic, Vercel formats) means both the backend provider and the frontend consumer can be swapped independently without changing the core system.

### Missing audit trails

Regulated industries (finance, healthcare, government) require complete audit trails for autonomous AI decision-making, particularly as the EU AI Act enforcement scales. No mainstream agent framework provides immutable, append-only audit logging as a core primitive. Lago’s event journal provides this by construction—it is architecturally impossible to delete or modify past events, and the content-addressed storage provides cryptographic verification of data integrity.

### Poor tooling governance at scale

MCP has 5,800+ servers  and growing,  but its governance model is immature. Enterprise deployments face the “Shadow Agent” problem: unauthorized agents accessing enterprise data through MCP servers without centralized policy control.  Lago’s RBAC policy engine provides a **centralized enforcement point** where organizations can define, version, and audit tool access policies across all agent instances.

## Five capabilities this architecture uniquely enables

### Long-lived agent sessions spanning days or weeks

Because all state derives from the event journal and filesystem state is branched and content-addressed, agent sessions are not bounded by process lifetime, memory limits, or context window size. An agent can work on a complex task over days, be stopped and resumed at will, and maintain full context through event replay and state projection. No existing framework supports this natively—**Replit Agent’s 200-minute autonomous sessions represent the current ceiling**,  and that uses snapshot-based persistence without event-sourced continuity.

### Time-travel debugging for non-deterministic AI systems

Given the append-only event journal, developers can reconstruct the exact state of any agent at any point in its execution history. This enables “time-travel debugging” where you can step forward and backward through an agent’s decision sequence, inspect the exact context it had when making each decision, and identify precisely where behavior diverged from expectations. This is qualitatively different from log analysis—it’s full state reconstruction with the ability to re-execute from any historical point.

### Branching agent workspaces for speculative reasoning

Lago’s Git-like filesystem branching enables agents to **fork their workspace**, explore a hypothesis (refactor this module, try this deployment strategy), evaluate the results, and either merge the successful branch or discard it entirely. This mirrors how human developers use Git branches for experimentation, but applied at the agent runtime level. Combined with the orchestrator’s iteration control, this enables a form of **Monte Carlo tree search over workspace states**—agents can systematically explore and prune solution spaces.

### Policy-governed autonomous agents for enterprise deployment

The combination of Arcan’s harness guardrails and Lago’s RBAC policy engine creates a **governance framework for autonomous agents** that enterprises can audit and trust. Tool access policies can be versioned, tested against historical event streams, and enforced consistently across all agent instances. Policy violations are recorded in the same event journal as successful actions, providing a complete audit trail that satisfies regulatory requirements. This governance model is a prerequisite for deploying autonomous agents in production environments with real business consequences.

### Multi-provider, multi-format agent execution

Arcan’s provider trait abstraction combined with Lago’s multi-format SSE compatibility means a single agent deployment can **switch LLM providers without code changes** and **serve events to clients built against different streaming protocols** (OpenAI, Anthropic, Vercel). This architectural flexibility de-risks agent deployments against provider pricing changes, API deprecations, or model capability shifts—concerns that industry surveys consistently identify as top barriers to production agent adoption.

## Why these technology choices and not others

### Rust over Python: safety as a system property, not a testing strategy

Every major agent framework is Python-first. Python’s dynamic typing, garbage collection, and GIL (Global Interpreter Lock) create three classes of problems in agent infrastructure: **type errors surface at runtime** (an incorrectly structured tool call crashes the agent mid-execution), **GC pauses create unpredictable latency** (problematic for streaming architectures), and **the GIL prevents true parallelism** (critical when orchestrating multiple tool executions simultaneously). Rust eliminates all three. Its ownership model provides memory safety without garbage collection.  Its type system catches event schema mismatches, incorrect state transitions, and API contract violations at compile time. Its async runtime (tokio) provides true concurrent execution without the GIL bottleneck.

The performance differential is not theoretical. As agent workloads scale—Cursor reportedly processes ~1 billion lines of code per day—  infrastructure overhead compounds. Rust’s **zero-cost abstractions** mean the orchestrator loop, event serialization, and SSE streaming add negligible overhead beyond the inherent cost of the operations themselves.  Zectonal’s production case study of building an AI agent framework in Rust validated the “genie-in-a-binary” deployment model: a single static binary with no runtime dependencies, deployable anywhere. 

### redb over PostgreSQL/SQLite: embedded ACID without operational overhead

External databases (PostgreSQL, Redis, SQLite) add deployment complexity, operational burden, and failure modes. redb is a **pure-Rust embedded key-value store** with ACID transactions, crash safety, and concurrent read access. Its copy-on-write B-tree semantics align naturally with append-only event journals—new events create new tree nodes without modifying existing ones, providing structural consistency guarantees. The zero-external-dependency property means Lago deploys as a single binary, can run in resource-constrained environments, and eliminates the “database is down” class of production incidents.

### Event sourcing over snapshot-based persistence

Snapshot-based persistence (used by Replit’s checkpoints, LangGraph’s checkpointing, Fly.io’s Sprite snapshots) stores point-in-time state. It answers “what state is the agent in?” but not “how did it get there?” or “what happened between these two states?” Event sourcing stores the **complete causal history**, enabling replay, auditing, debugging, and testing capabilities that snapshots cannot provide.  The append-only constraint also simplifies distributed consistency—events flow in one direction and are never modified, eliminating entire categories of concurrency bugs.

### Content-addressed storage over path-based file systems

Path-based file systems (the default in every agent runtime) identify files by location, not content. This means identical files are stored redundantly, integrity cannot be verified without full re-reads, and diffing requires content comparison. **SHA-256 content addressing** provides deduplication by construction, integrity verification by hash comparison, and efficient diffing by hash comparison alone. Combined with zstd compression, this minimizes storage footprint while maintaining cryptographic integrity guarantees—essential for the audit trail requirements of production agent deployments.

### gRPC + SSE dual protocol over single-protocol architectures

Internal agent-to-persistence communication requires **high throughput, low latency, and bidirectional streaming**—gRPC via tonic is purpose-built for this. External client consumption requires **broad compatibility, firewall-friendliness, and simplicity**—SSE over HTTP provides this. Running both protocols simultaneously means Arcan and Lago communicate internally at native gRPC speeds while remaining accessible to any HTTP client externally. The multi-format SSE support (OpenAI, Anthropic, Vercel compatible) maximizes frontend ecosystem compatibility without protocol translation layers.

## Conclusion: the shift from agent frameworks to agent operating systems

The Arcan + Lago stack represents an architectural thesis that the AI agent ecosystem will mature from frameworks (libraries you import into your application) to **operating systems** (infrastructure that manages the lifecycle, persistence, security, and governance of agents as first-class entities). This mirrors the historical arc of web development: from CGI scripts to application servers to container orchestration platforms. The “Agent OS” concept means agents get process isolation (the harness), a filesystem (lago-fs with branching), persistent storage (the event journal), access control (the policy engine), and inter-process communication (typed streaming events)—the same primitives that operating systems provide to applications.

No other system in the current landscape combines Rust’s compile-time safety guarantees, event-sourced persistence with time-travel debugging, content-addressed storage with cryptographic integrity, Git-like workspace branching, RBAC policy enforcement at the persistence boundary, and multi-format streaming compatibility in a single, zero-external-dependency stack. The common weaknesses identified across all major frameworks—security gaps, state management fragmentation, missing audit trails, poor governance tooling, and ephemeral execution—are precisely the problems this architecture was designed to solve.

The market conditions are aligned: **$3.8 billion** flowed into AI agent startups in 2024 alone (3× the previous year),  enterprises identify security as their top deployment barrier,  regulators are mandating audit trails for autonomous AI systems, and the framework fragmentation across LangChain, CrewAI, AutoGen, and others is creating demand for infrastructure that is protocol-native rather than framework-specific. Arcan + Lago is positioned not as another framework in this crowded field, but as the infrastructure layer beneath all of them—the persistence, security, and governance substrate that production-grade autonomous agents require and that no existing system provides.




Lago + Arcan Agentic Runtime Architecture

Architectural Layers and Responsibilities
	•	User Interface (Next.js + Vercel AI SDK): The frontend is built with Next.js and Vercel’s AI SDK for chat/agent UIs ￼.  It captures user input, displays streaming LLM responses (via hooks like useAIChat), and forwards requests to the backend.  The AI SDK abstracts away LLM calls and streaming logic, handling details like model selection, streaming responses, and connecting tool outputs back into the conversation ￼.
	•	Agent Runtime (Arcan Harness): Arcan is the core orchestrator.  It loads persona/context (e.g. from “soul” or identity files), compiles the current prompt, invokes the LLM, detects structured tool calls, executes skills, and enforces policies.  Like other agent harnesses, it compiles a working context for each LLM turn that includes relevant history, facts, and results ￼.  Arcan also intercepts special model outputs (tool calls) and routes them to external tools/skills ￼. After each step it logs outputs, updates memory, and iterates or completes.
	•	Data Plane (Lago Event Store): Lago serves as the long-term storage and memory for the agent.  It event-sources every interaction and change: messages, observations, tool calls/results, memory writes, etc.  In other words, each change to the agent’s state is logged as an immutable event with a timestamp and details ￼.  This provides a full audit trail (“journey”) of the agent’s knowledge and actions.  Data is organized per agent or “workspace” so that each agent’s logs and memory are isolated (similar to how Unity Catalog segments data by workspace ￼).  The raw events can be stored on a data lake (e.g. S3/ADLS) using an open table format like Delta Lake or Iceberg ￼ to get ACID guarantees and schema.  A unified catalog (e.g. Unity Catalog) sits above this lakehouse to manage metadata, enforce access control, and track data lineage across all agents/workspaces ￼.
	•	Tools/Skills Layer (Skills.sh model): Skills are treated as first-class objects.  Each skill is a modular package with a declarative schema (inputs, outputs) and its implementation (script, binary, or WASM).  This follows the skills.sh￼ pattern: “Each skill follows a simple contract that defines its inputs, outputs, and execution behavior” ￼.  Skills include human-readable descriptions (used to prompt the model) and strict type schemas (JSON Schema/Zod) for inputs.  At runtime, Arcan matches a model’s tool-call request to a skill, validates the arguments against the schema, executes the skill (in a sandbox), and records the call/result ￼ ￼.  Because skills are versioned and auditable, the system separates “reasoning” (LLM planning) from “execution” (running trusted code) and logs all tool usages for review.
	•	Governance & Security Layer: This layer enforces policies and monitors the system.  All actions (LLM outputs, tool calls, memory updates) are logged for traceability.  Guardrails (quotas, content filters, user approvals) are applied between the “brain” and the “hands.” For example, OpenClaw’s design uses budgets (“no endless loops”), approvals (“user ok for big actions”), and audits (“log everything”) as guardrails ￼.  In our design, a policy engine (e.g. Open Policy Agent) evaluates every proposed action or memory update against organizational rules ￼.  Dangerous actions are either blocked or flagged for human review.  Tools/skills themselves run in secure sandboxes (e.g. WebAssembly via Wasmtime) to isolate the agent’s execution from the host system ￼.  Unity Catalog’s audit logs and the event store ensure data lineage and compliance across agents ￼.

Event-Sourced Memory & Workspace Separation

All agent state is built from events in Lago.  In event sourcing, every change is an append-only event ￼.  For example, a user message, an LLM reply, a tool invocation, or a memory update each becomes a timestamped event.  By replaying these events, one can reconstruct the full conversation, memories, and actions of the agent.  This also gives rich analytic data for retraining or debugging: we keep the journey, not just the snapshot ￼.

Memory in this plane can be multi-tiered.  Recent conversation turns are kept in fast storage (an in-memory cache like Redis ￼ as a short-term “session memory”).  Long-term memories are stored as vector embeddings (semantic memory) and knowledge graphs (like Mem0).  For example, we can adopt Observational Memory from Mastra: split memory into “observations” (concise summaries) vs raw logs.  A background observer agent compresses conversation logs into stable memory units, and a reflector prunes irrelevant memories ￼.  This lets us maintain a working context without exceeding token limits.  In parallel, a Graph Memory (e.g. Mem0) builds a semantic graph of entities and their relations ￼.  When we retrieve memories, we can return not just raw text but also related concepts from this graph to enrich context.

Workspace separation: Each agent instance (or user workspace) has its own event stream and databases. Data catalogs (like Unity Catalog) can enforce this multi-tenancy by partitioning data per workspace and applying policies ￼.  For example, an agent’s personal “soul”, identity, and past interactions live in its private namespace.  From a governance perspective, every event is tagged with agent/workspace ID and audited through the unified catalog.

Skills as Lago Artifacts

In our design, skills are stored and managed by Lago as first-class artifacts.  Each skill directory contains: a schema file (e.g. JSON or Zod schema) defining inputs/outputs, a description/prompt template, and the executable code (script or WASM module).  This mirrors the skills.sh philosophy that “skills are described using simple configuration files” ￼.  For example, a skill for weather might have a schema { location: string } and code that calls a weather API.  The agent runtime (Arcan) loads all available skill schemas at startup.  When the LLM outputs a skill invocation, Arcan automatically extracts the tool name and arguments and validates them against the skill’s schema ￼.  It then runs the skill (in a sandbox) and logs both the call and result.  This ensures agents call skills in a predictable, auditable way ￼ ￼.  Skills are versioned, so teams can review and update them safely without altering core agent logic.

Self-Improving Agent Logic (Proposals, Commit Gate, Policy Enforcement)

To support learning and adaptation, the agent can propose changes (e.g. adding a new fact to memory, refining its own code, or adjusting a workflow).  These proposals do not take effect immediately; they go through a commit gate.  The commit gate consists of policy checks and (optionally) human approval.  For instance, if the agent decides “remember this new user preference,” that memory entry is held as a draft event until a policy engine or moderator signs off.  This prevents unwanted or unsafe updates.

Behind the scenes, we implement an intermediate protocol layer as described in Micheal Bee’s architecture for self-improving agents: a set of operational protocols that describe behaviors and can be tracked ￼.  A monitoring harness logs each protocol access (which memory key was added, which rule fired, etc.) ￼. Over time we build heatmaps and usage statistics to see which actions the agent really needed ￼.  From this data, we can evolve the system (e.g. promote frequently-used routines into core tools).  Throughout this process, policy enforcement acts as a guardrail: budgets, rate limits, content filters and manual overrides keep the agent aligned ￼. In practice, this means every proposed memory write or code generation is validated by OPA rules or human review before being committed to the Lago store.

Arcan Harness Responsibilities

The Arcan harness is responsible for context compilation, tool management, and policy enforcement:
	•	Context Compilation: Before each LLM call, Arcan gathers relevant information into the prompt.  It retrieves recent conversation turns, persona files (loaded at startup like OpenClaw’s soul.md and identity.md ￼), and fetched memories.  The harness ensures the model sees what it needs: it summarizes or omits old data to avoid context overflow ￼.  This “working context” is a curated prompt that includes essential facts and recent results ￼, allowing the agent to work on tasks spanning beyond a single session.
	•	Tool/Skill Management: The harness monitors the LLM’s output for structured tool-call tokens. When a tool call is detected, Arcan pauses the LLM, locates the corresponding skill, validates inputs against its schema, and executes it ￼ ￼.  After execution, the result is fed back into the agent’s conversation context for further reasoning.  In this way, Arcan effectively gives the LLM “hands and eyes” to act on the world, just as described in agent harness literature ￼ ￼. Default tools (file I/O, web search, code execution) can be provided out-of-the-box, and custom skills added via Lago.
	•	Policy Gate (Verification & Guardrails): Arcan enforces safety and correctness. It validates every output and action: for example, JSON schemas ensure tool outputs are well-formed, unit tests can verify generated code, and content filters block disallowed responses.  This follows the “verification and guardrails” role of a harness ￼.  Any rule violation (e.g. a forbidden API call) triggers a policy exception.  Budgeting and approvals operate here as well: the harness tracks token usage and loop iterations, halting if limits are exceeded ￼. All actions and decisions are logged for audit, aligning with Unity Catalog’s lineage features.

File Layouts, Event Types, and Runtime Flows

Suggested File Layout: Organize code and data by function. For example:
	•	/arcan/ – Arcan harness code (Rust)
	•	agents/ – agent logic, planners, protocols
	•	tools/ – local tool implementations (or hooks to skills)
	•	policies/ – Rego files for OPA policy enforcement
	•	/lago/ – data plane definitions
	•	schemas/ – event and database schemas (messages, memories, etc.)
	•	migrations/ – SQL or scripts to set up Delta/Iceberg tables
	•	/skills/ – skill packages (each skill is a subfolder with schema and code/WASM)
	•	/frontend/ – Next.js app using Vercel AI SDK
	•	components/ – chat UI, etc.
	•	pages/api/ – API routes that call the Arcan backend

Event Types: Define a clear set of event types for Lago. For example: MessageSent, MessageReceived, MemoryAdded, SkillInvoked, SkillResult, PolicyDecision, etc.  Follow an observability standard: each span or event in AOS (Agent Observability Standard) maps to a step.  For instance, steps/toolCallRequest events record tool ID and inputs ￼, and steps/memoryRetrieval events log memory queries and contents ￼. These events feed into Lago’s tables and OpenTelemetry traces.

Runtime Flow: A typical run might look like:
	1.	User Input → Harness: The frontend sends user input; the harness logs a MessageReceived event.
	2.	Context Assembly: Arcan fetches recent memory and persona, composes the prompt.
	3.	LLM Invocation: The LLM is called. Output is either final text or a tool call.
	4.	Tool Execution: If a tool call (e.g. weather(city)) is returned, Arcan logs a ToolCallRequest event, invokes the skill sandbox, then logs ToolCallResult.
	5.	Memory Proposal: The agent may decide to store new information. It creates a MemoryProposal event, which goes through the commit gate (policies). If approved, a MemoryAdded event is written to Lago.
	6.	Iteration: The result of the tool (and any additional LLM steps) is appended to the conversation and returned. A MessageSent event logs the agent’s reply. If multi-step reasoning is allowed, loop back to step 3 with updated context.
	7.	Tracing: Throughout, each step emits telemetry spans (e.g. span:agent.run, child spans for each turn, each tool call, etc.), following AOS conventions for full visibility ￼ ￼.

This structured flow ensures every action is accounted for and can be traced or audited after the fact.

Libraries and Toolchains
	•	Vector Databases: For semantic memory/retrieval, use dedicated vector DBs. Common choices include Pinecone, Weaviate, Milvus, Qdrant, or PostgreSQL with pgvector ￼. These store high-dimensional embeddings of agent knowledge for similarity search.
	•	WebAssembly Runtime: To sandbox skills and untrusted code, use a WASM engine like Wasmtime￼.  For example, Microsoft’s Wassette project uses Wasmtime to run WebAssembly components as agent tools, with a fine-grained permission model ￼. Arcan can invoke WASM skills via the Model Context Protocol or a CLI, ensuring isolation.
	•	Tracing / Observability: Instrument the harness and Lago with OpenTelemetry.  Follow the Agent Observability Standard to emit spans for each agent step ￼ ￼.  This means recording spans for user messages, LLM calls, tool calls, memory fetches, etc., with attributes for inputs/outputs and agent reasoning.  Aggregated traces help diagnose performance, identify bottlenecks, and verify policy adherence.
	•	Policy Engine: Use Open Policy Agent (OPA) for policy-as-code.  OPA offers a declarative language (Rego) and a fast decision point ￼.  The harness calls OPA on each proposed action or memory update.  OPA evaluates rules (e.g. no disallowed API calls, content filters, rate limits) and returns allow/deny.  Its audit logging can feed back into Lago for accountability.
	•	Session Memory Cache: For short-lived session state, use an in-memory store like Redis or Memcached ￼.  This holds the last few messages or intermediate context for quick prompt-building. It complements the durable Lago store by handling rapid read/writes at millisecond latency.

Each of these libraries fits into the stack: vector DBs and WASM runtimes interface with Lago/Arcan, tracing is integrated across all components, and OPA plugs into the harness’s workflow.

Sources: This design draws on recent AI agent research and tools: OpenClaw/Pi for persona and sandbox ideas ￼ ￼, Mastra/Mem0 for memory models ￼ ￼, data lakehouse best practices ￼ ￼, skills.sh for modular tools ￼ ￼, and Vercel’s AI SDK/Next.js for frontend integration ￼ ￼. These components together form a cohesive, production-ready agentic runtime. Each layer and flow is designed for scalability, auditability, and continuous improvement.



Arcan & Lago: A Rust-Based AI Agent Operating System with Web3 Identity Integration

Arcan is a Rust-first agent runtime and daemon designed as an “AI-native meta-platform” to build, deploy and orchestrate full-stack AI agents ￼ ￼.  Its core features include harness quality (sandboxing and guardrails), typed streaming events, and a replayable state (event-sourced logs) ￼ ￼.  In practice, Arcan treats AI agents like processes on an operating system: it provides an orchestrator loop (the arcand daemon) to coordinate agent actions, a typed event stream (via Server-Sent Events over HTTP), and strong safety measures (via the arcan-harness sandbox).  Uniquely, Arcan weaves in Web3 identity and privacy: it lets developers “tie AI personalization to user-owned blockchain profiles” so that AI agents can deliver customized services under user control ￼ ￼.  In short, Arcan aims to be a unified agent OS that not only runs LLM-driven agents but also handles blockchain-backed user profiles, data encryption, and end-to-end event tracking.

The Arcan workspace consists of several Rust crates, each handling a subsystem ￼: for example, arcan-core implements the protocol, state types, and the orchestrator loop; arcan-harness provides a secure sandbox and filesystem guardrails; arcan-store is an append-only event repository for agent session logs; arcan-provider plugs in LLM backends (e.g. Anthropic’s Claude); arcand runs the main agent loop and exposes an SSE streaming server + HTTP API; and arcan-lago bridges Arcan to the Lago event-sourcing platform.  The CLI binary (arcan) ties it all together (portable via cargo install arcan).  For example, one can run the agent system with a mock LLM or with a real API key:

# Run with mock provider
cargo run -p arcan

# Run with Anthropic Claude
ANTHROPIC_API_KEY=sk-… cargo run -p arcan

Arcan’s design emphasizes reproducibility and audit: every agent step is logged in a typed event stream so that entire agent sessions can be replayed or audited later.  (Indeed, Arcan’s own daemon streams events over SSE, so front-end UIs or monitoring tools can subscribe to the live event feed.)

Architecture: Agent Loop, Harness, and Event Store

Conceptually, Arcan acts like an “agent OS” or orchestrator for AI workflows.  It coordinates multiple specialized agents in a unified system, much as described in AI orchestration literature.  For example, IBM defines “AI agent orchestration” as coordinating a network of specialized AI agents within a single system to efficiently achieve complex goals ￼ ￼.  In Arcan, the arcand crate embodies this orchestrator: it runs a loop that invokes agents (via LLM providers), ingests their tool-outputs, and streams the results as typed events.  The orchestrator is itself extensible, so multiple agents (or chains of agents) can be composed.  As IBM notes, having an orchestrator that synchronizes “the right agent at the right time” is key to handling multifaceted workflows ￼.  In practice, Arcan’s loop can invoke different agents (defined by code or prompts) and integrate their results; it exposes that flow over HTTP/SSE so operators can supervise or intervene.

The harness is a standout feature of Arcan’s architecture.  The arcan-harness crate implements sandboxing and guardrails around agent execution.  This means user code (or tools) run in a controlled environment: e.g. file system access or side-effects are mediated by hashline edit primitives.  In effect, Arcan can prevent unintended behavior by agents (for instance, by intercepting filesystem writes or restricting network calls) while still recording all attempted actions.  This “defense-in-depth” is atypical for LLM frameworks: many open-source agent tools focus only on prompt logic, whereas Arcan embeds the agent loop in a secure Rust runtime ￼.

State management in Arcan is event-sourced.  The arcan-store crate maintains an append-only log of all session events (user inputs, agent outputs, tool calls, etc.).  This design ensures that the entire history of an agent session is stored durably and can be replayed to rebuild state or debug behavior.  The optional arcan-lago crate connects Arcan to the external Lago platform (an open-source usage/event store) for large-scale persistence.

These architectural pieces combine into a coherent loop: when an agent runs, each step is sent through the harness (for safety), passed to the LLM provider or tool, and the result is appended to the event store and pushed out via SSE.  A watcher (CLI UI, dashboard, or another process) can listen to SSE or query the store to see each decision.  This setup supports multi-agent workflows and human-in-the-loop if desired.  (If multiple agents coordinate, an orchestrator in Arcan can delegate subtasks to specialized agents, much like “CrewAI” or “Autogen” frameworks that form agent teams ￼.)  Crucially, the typed-event approach means that all intermediate data (like JSON responses or function calls) are explicitly schema-checked by Rust types, reducing runtime surprises.

What Problems Arcan Solves and Possibilities

Arcan addresses several pain points in AI agent development.  Reproducibility and debugging: By design, Arcan’s agents produce a fully replayable log.  This solves a common issue where many AI workflows are opaque – with Arcan, every input, API call, and decision is recorded.  Engineers can rewind and replay agent runs to trace errors or verify behavior, which is far more robust than ephemeral LLM chats.  Security and correctness: The sandboxed harness helps prevent misbehaving agents from corrupting data or leaking secrets.  It enforces constraints at the OS level, something few Python-based frameworks offer out of the box.  Typed, stream-based communication: All agent interactions are modeled as strongly-typed Rust data.  This dramatically reduces type errors (e.g. JSON serialization issues) and makes it easier to serialize complex events, again aiding audit and integration.  Customization and user ownership: By integrating blockchain profiles, Arcan lets AI personalization be owned by users ￼.  For example, one could store a user’s preferences or credentials on-chain and use them in agent prompts only with explicit permission – a level of privacy and ownership often missing in centralized AI services.  As Arcan’s PyPI description emphasizes, this yields a “privacy-centric design” where data is encrypted and under user control ￼.

These features open up new possibilities.  Teams can deploy Arcan agents into regulated or collaborative environments knowing that every step is auditable (comparable to the stringent orchestration envisioned by IBM and PwC for enterprise agents ￼ ￼).  The persistence layer (especially with Lago) means one could build analytics on agent usage, drive billing for AI services, or replay learned behaviors.  The Rust implementation also allows running Arcan itself in resource-constrained or high-performance settings (unlike most Python frameworks).  In short, Arcan aims to be an “agent operating system” that brings software engineering discipline (type safety, testing, reproducibility) and Web3 integration to the rapidly growing field of AI agents.

Market Landscape and Differentiators

AI agent frameworks are rapidly emerging.  By 2025, industry analysts note many open-source frameworks (LangChain, LangGraph, AutoGen, CrewAI, etc.) that help build agent workflows ￼ ￼.  These tools generally allow connecting LLMs to prompts and tools with some orchestration logic, but they are usually Python-centric and loosely-typed.  For example, LangChain is “a versatile framework for developing and deploying AI applications powered by LLMs” ￼, widely used for NLP pipelines.  Arcan differs fundamentally by being Rust-based, emphasizing static types and low-level control.  Unlike LangChain or LlamaIndex (which focus on data retrieval and prompt chaining), Arcan is designed for complex agent workflows with strong guarantees (sandboxing and event logs).

Moreover, while many frameworks focus only on AI logic, Arcan explicitly integrates Web3 tooling and blockchain.  Few alternatives offer on-chain identity or asset management out of the box.  Arcan’s value is in enabling decentralized AI personalization: agents can interact with blockchain data (NFTs, tokens, contracts) tied to user profiles, opening new use cases in digital identity and decentralized finance.  In essence, Arcan combines the “AI agent OS” concept touted by consultancies like PwC ￼ with open-source flexibility and cryptographic trust.

In summary, Arcan (with the Lago bridge) provides a unique stack: a Rust-powered orchestrator loop (arcand), a secure execution harness, an event-sourced state store (backed by Lago for scale), and blockchain-aware data handling.  This creates an AI agent platform where workflows are reproducible, auditable, and user-centric.  The result is faster, safer agent development and novel AI applications (e.g. personalized assistants that respect user ownership).

Sources: Arcan’s design and features are documented in its GitHub repository and PyPI page ￼ ￼ ￼.  Industry context on AI agent orchestration and frameworks is drawn from IBM and AI21 analyses ￼ ￼, and PwC’s description of “agent OS” platforms￼. These references inform the above technical analysis of Arcan+Lago’s architecture and value.