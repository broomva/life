# Spec I — Khora: Agent Environment Substrate

**Date**: 2026-05-16
**Author**: Carlos Escobar + Claude (Sonnet 4.7, 1M context)
**Status**: ⛔ **v1 FAILED P20 cross-review (2026-06-04)** — redesign pending strategic decision. See §0.
**Sibling specs**: Spec C (life-runtime), Spec D (Anima Custody), Spec E (InferenceBackend)
**Research substrate**:
  - `research/notes/2026-05-15-agent-sandbox-substrates-raw.md` — Vercel Sandbox, E2B, Firecracker vs gVisor vs WASM vs V8 isolates substrate comparison
  - `research/notes/2026-05-15-aci-thesis-literature-synthesis.md` — SWE-agent, Anthropic Computer Use, Cognition Devin, the "filesystem + bash" thesis primary literature
**Linear umbrella**: pending file (draft text at §11.1)

---

## 0. P20 Cross-Review Verdict — ⛔ FAIL (v1, 2026-06-04)

This spec was put through a **P20 cross-model adversarial gate** before any code. Four independent reviewers — one cross-vendor (Codex / GPT-5, which web-searched the Firecracker snapshot docs to verify), three fresh-context Claude lenses (substrate, security, product) — were briefed to *refute, not praise*.

| Reviewer | Lens | Anti-slop (≥7 to pass) | Verdict |
|---|---|---|---|
| Codex / GPT-5 (cross-vendor) | whole spec | **4/10** | FAIL |
| Architect | substrate / distributed | **4/10** | FAIL |
| Silas | security / capability | **3/10** | FAIL |
| general-purpose | product / strategy | **3/10** | FAIL |

**Mean 3.5/10 — decisive FAIL. Do NOT start Sub-A as written.** The gate worked: a polished, plausible v1 had several load-bearing decisions that are wrong or unbuildable.

### Consensus blockers (named independently by ≥2 reviewers, incl. the cross-vendor leg)

1. **Branch is mechanically invalid as specified.** You cannot `cp --reflink` a *live, un-paused* Firecracker memfile and keep the parent running — Firecracker requires `Pause → CreateSnapshot`, and loaded memory files must be immutable. Honest mechanism: `pause(parent ~ms) → snapshot → reflink the frozen snapshot → N× restore`. The "<50ms, parent keeps running, live fork" headline (L7-D6) is false. *(all 4)*
2. **Cloned-memory divergence unhandled.** Children booted from one image share RNG/entropy, TCP/socket state, TSC, fds, MMDS identity → corrupted children + key/nonce collisions, absent a mandatory post-restore divergence hook (reseed CSPRNG via VMGenID, tear down sockets, rotate identity) before guest code runs. *(Architect, Codex, Silas)*
3. **MintEgressToken is a confused deputy + the TLS model is contradictory.** The capability binds `user_id`+`scope`, never a destination host — a hostile guest names an allow-listed host (wildcard / redirect / SSRF / DNS-rebind) and the proxy injects the *real* credential into the attacker stream. Header injection also requires TLS-termination (proxy sees every tenant's plaintext secrets), contradicting "mTLS/SNI". The component doesn't exist. *(Codex, Silas)*
4. **"Secured by default, no opt-out" is enforced nowhere.** No default-deny network namespace; a guest with a raw socket bypasses an app-layer proxy (raw TCP/UDP/DNS/QUIC). *(Codex, Silas)*
5. **Snapshot / branch leaks secrets.** A paused snapshot = a memory dump in Lago CAS (by-hash, broadly readable, not stated encrypted); branch children inherit the parent's *live egress token*. *(Silas, Codex)*
6. **L7-D3 CAS-for-memfile defeats UFFD.** You cannot lazily page-fault out of a zstd blob — you must inflate the whole GB-scale memfile first (seconds), killing the warm-pool target. Memfile must live local + uncompressed on NVMe; CAS is cold archive only. *(Architect)*
7. **Cross-host resume (AC#4) contradicts the spec's own OQ#7** and is unsafe without CPU-feature-pinned host pools (TSC/AVX). *(Codex, Architect)*
8. **L7-D2 "reuse life-runtime-pool" is an impedance mismatch** — it's a gRPC *channel* pool, not a stateful-VM lifecycle pool. *(Architect)*
9. **Five ACI primitives are insufficient** — missing process control (signal/kill), PTY/TTY resize, port-forward, `MountVolume`, structured env/secret injection. PID-1 envd also understates the init contract (zombie reaping, signal forwarding, pseudo-fs mounts). *(Codex, Architect)*
10. **Phasing is 3–5× optimistic** — 15 days for a from-scratch FC orchestrator + UFFD page server + template pipeline + egress proxy + conformance is not credible; realistic floor 6–10 weeks. *(all 4)*

### Strategic findings (the "should we build this, now, this way" lens)

11. **Build-vs-buy ignored.** E2B's infra is Apache-2.0 and already ships FC + UFFD + templates + orchestrator. The only cited reason to rewrite is "Rust vocabulary." Wrapping/forking E2B reclaims ~12 of 17 days; the cross-substrate fabric (Lago/Haima/Vigil/Anima sinks) is event-emission at the orchestration layer — achievable *over* a wrapper. *(Product, Codex)*
12. **Mis-sequenced.** Spec E (the explicitly *paired* compute contract) is 17%; M9 apps/chat migration is the real critical path; nothing in v0.2.0 is blocked on OS-isolation. Khora's own GPU/exec path depends on Spec-E milestones that don't exist yet. *(Product)*
13. **Three competing sandbox layers** — praxis soft-sandbox + planned Aegis + Khora, no consolidation plan. L7-D5 *is* policy, pre-empting Aegis's charter. *(Product)*
14. **Branch may be a solution looking for a problem** — "no competitor ships it" is at least as likely a no-demand signal as a moat; the spec itself flags the core mechanism as unsolved. *(Product)*

### Disposition

Blockers 1–9 are **correctable in a redesign** (the substrate/security decisions were wrong, not the ambition). Findings 11–14 are **strategic forks that are the principal's call** — they are surfaced for decision, not resolved here. A v2 spec is gated on that decision. Full review artifacts: Linear BRO-1377 (verdict comment); Codex transcript `/tmp/khora-p20-codex-out.md`.

---

## 1. Problem Statement

Life's agent loop today calls into a **soft sandbox**. Per `core/life/CLAUDE.md` (Known gaps, v0.2.0):

> - No OS-level sandbox isolation (soft sandbox only)
> - Network isolation declared but not enforced
> - Mount trait defined but unimplemented

`crates/praxis/praxis-core/` ships `FsPolicy` (workspace boundary check at the path-string level) and `SandboxPolicy` (command-name allowlist). Both are advisory — they assume a cooperating agent. A malicious or buggy tool call can:
- Read any file the daemon's UID can read
- Make arbitrary egress requests (network policy declared in policy.yaml is not enforced at the kernel)
- Consume unbounded CPU and RAM
- Persist arbitrary state across sessions
- Cannot pause-and-resume — long-horizon agents (>1h) accumulate context and either retry-from-scratch or block the host

The praxis path was correct for v0.1 (proving the agent loop). It is the wrong substrate for production multi-tenant agent hosting, for long-horizon work, for speculative parallel exploration, and for any deployment where the agent code is not fully trusted (chatOS user sessions, mission-control third-party skills, the `freelance/_template` Life Module pattern, Phronesis advisory-as-a-service).

The market answers (Vercel Sandbox, E2B, Modal, Daytona, Runloop, Northflank, Cloudflare Sandbox) converge on **Firecracker microVMs + UFFD lazy memory restore + per-host orchestrator + guest gRPC daemon** as the de facto reference architecture for 2025–2026 agent sandboxes (research note §5). Life does not have this substrate. Spec I builds it.

---

## 2. Vision: Khora as the Receptacle

> *"…there is a third nature, which is space (χώρα), and is eternal, and admits not of destruction and provides a home for all created things…"*
> — Plato, *Timaeus* 52a–b

Plato's *khora* (χώρα) is the **receptacle of becoming** — neither the eternal Forms nor the sensible particulars, but the third kind, the *place* in which forms imprint themselves into matter. Aristotle later read it as the indeterminate substrate, the *room* in which things come to be. Derrida, two millennia on, read khora as the place that exceeds binary oppositions — at once active and passive, formative and form-receiving.

For Life:

- **Anima** = the agent's *soul* (immutable origin, mutable belief, cryptographic root). What the agent is.
- **Khora** = the agent's *place of becoming*. Where the agent acts, fails, snapshots, branches, persists, terminates. The substrate in which the agent's history *happens*.

The pairing is precise. Anima alone is a static document. Khora alone is an empty room. Together they describe an agent that *exists somewhere doing something*. Every Life primitive sits between or around these two:

```
soma   — the body (kernel daemon, custody oracle, admin plane)        [shipped: M0/M1]
anima  — the soul (identity types + trait + 6 custody backends)        [shipped: Spec D]
khora  — the place of becoming (agent environment substrate)           ← THIS SPEC
aegis  — the shield (capability/secret/attestation policy)             [planned — composes WITH khora]
haima  — the blood (per-vCPU-second / per-GB-net metering)             [active]
vigil  — the watch (OTLP spans for Exec/Fs/Net + Firecracker metrics)  [active]
lago   — the lake (event sink for every khora.* action)                [active]
chronos — time (pause TTL, snapshot expiry, idle scheduling)            [shipped: M0]
nous   — mind (scores tool-call observations from khora)               [active]
opsis  — sight (world-state seen through khora's window)               [active]
praxis — action (tool execution; runs *inside* a khora)                [active]
inference — silicon (compute backends; consumed *by* khora's exec path) [partial — Spec E E-Sub-A]
```

Khora is **not** Aegis. Aegis is the *policy* applied to khora — what capabilities a given khora may hold, what egress it may reach, what attestation proves it. The shield around the place. Both are needed; Spec I delivers the place. Aegis remains in the roadmap as the policy primitive that composes on top.

---

## 3. Core Thesis

**Three claims this spec rests on.** Each is grounded in the research substrate; each is independently falsifiable.

### 3.1 Environment dominates capability

The claim that *agent environment quality bounds agent performance more than raw model capability* has three independent supports:

1. **Anthropic Computer Use (Oct 2024)**: *"Instead of making specific tools to help Claude complete individual tasks, we're teaching it general computer skills."* Three primitives — `computer_*`, `text_editor_*`, `bash_*` — generalize broadly. The environment carries the structure; the model finds the path. (research note §3.2)
2. **SWE-agent (arXiv:2405.15793, Yang et al.)**: A *tuned, small* Agent-Computer Interface beats a baseline scaffold on SWE-bench Lite. The paper posits LM agents as *"a new category of end users"*. (research note §3.1)
3. **Industry signal**: Anthropic spending $1B+ on RL environments (TechCrunch 2025-09-21); Surge / Mercor / Mechanize / Prime Intellect built a market around environment quality. (research note §3.6)

Khora's design follows: invest in environment, not in agent surface area.

### 3.2 The ACI is filesystem + bash + branch

The strong claim *"filesystem and bash is all you need"* is **not** in the primary literature in that form — SWE-agent's claim is narrower (tuned-and-small beats fat). But the *practical* convergence is real: Vercel Sandbox, E2B, Devin, Anthropic Computer Use, SWE-agent all expose roughly `{exec, fs.{read,write,edit,list}, net.policy}`. Khora ships that set as **five ACI primitives** plus four lifecycle primitives (`Create`, `Pause`, `Resume`, `Branch`, `Stop`). Anything else (Jupyter, browser, GPU compute, MCP servers, language servers) runs *inside* the khora via `Exec`.

The novel primitive — `Branch` — comes from Life's existing journal-branching pattern (`BranchId::main()` at `crates/arcan/arcand/src/canonical.rs:1265`). The mental model already exists for the event store; Spec I extends it to the *runtime environment*. A khora can fork into N children cheaply (COW overlayfs + memfile clone), each child diverges, parent or merge-keep-best selects.

### 3.3 The sandbox is part of the agent's body, not a remote service

The agent loop in `arcand::canonical::run_session` already binds an `Arc<dyn AnimaCustody>` at session start. Spec I adds a peer: `Arc<dyn KhoraSession>`. Both are *part of the agent* — they are not external services the agent *calls*; they are surfaces the agent *has*. This is the difference between Vercel Sandbox (sandbox is a service the function calls) and Khora (the agent *is* the (anima, khora, …) tuple at runtime).

Operationally this matters because:
- The same khora persists across the 8-phase tick (`Perceive → Deliberate → StateEstimated → body → Commit → Reflect → Sleep|Recover`); state survives between turns
- Pause / resume preserve the *agent's working state*, not just a function's call frame
- Branch produces *parallel agents* with shared origin, not parallel function calls

---

## 4. Locked Design Decisions (L7-D1 .. L7-D8)

These mirror Spec D's L4-D1..D10 and Spec E's L5-D1..D8 — invariants that downstream sub-phases inherit and cannot relitigate without a spec amendment.

### L7-D1: Firecracker primary; Cloud Hypervisor sibling pool for GPU only

**Decision.** Khora's outer isolation is Firecracker microVMs (KVM-backed, AWS-shepherded, Apache-2.0). A sibling pool of Cloud Hypervisor instances exists *only* for the GPU-needed tier (Spec E inference workloads via VFIO passthrough). Inner tool-eval sandboxing (deterministic helpers, read-only ops, language-server queries) uses **wasmtime** to avoid the microVM cost for sub-millisecond calls.

**Rationale.** Firecracker has the only production-grade snapshot/restore primitive at agent-sandbox scale (p50 78ms warm-pool restore at E2B; ~28ms in benchmarks; <5MiB/VM overhead; ~150 VM/s/host) and is what every credible competitor uses. gVisor's 18% typical / 125% worst-case syscall tax (research §1.b) disqualifies it for filesystem-heavy agent workloads; gVisor's lack of production checkpoint/restore disqualifies it for resume-on-new-host. Cloud Hypervisor matches Firecracker on isolation, beats it on GPU passthrough, and shares the same VMM lineage — natural sibling.

**Alternatives rejected.**
- *Pure gVisor* — 125% worst-case overhead on SQLite-style workloads (research §1.b); no production CRIU.
- *Pure WASM (wasmtime + WASI Preview 2)* — no subprocess model; WASI Preview 3 async not until 2026/27 (research §1.f); agent cannot run real `bash`, `python`, `node`.
- *V8 isolates* — no filesystem, no subprocess, 128MB cap; disqualified for any non-script-only workload.
- *Kata Containers* — heavier than Firecracker, no agent-specific wins; fine for K8s but Khora doesn't need K8s at VM layer.
- *Pure container (Docker/runc)* — kernel-shared, weak tenant isolation, no snapshot/restore primitive.

### L7-D2: Per-host Rust orchestrator (`khorad`); Nomad/K8s schedules host pool only

**Decision.** A single Rust daemon (`khorad`) per VM host owns the Firecracker fleet on that host: VM lifecycle, UFFD page server, template store cache, egress proxy. Nomad / K8s / EC2-ASG schedules *hosts*, not VMs. The orchestrator is Rust because the rest of life-runtime is Rust (Spec C's `lifed`, `lifegw`, `life-runtime-pool`, `life-runtime-proto`) and the operational surface (pool, breaker, saga, OTLP) is already mature.

**Rationale.** E2B chose Go because they predate `life-runtime-pool`; Life has the pool + breaker + half-open CAS shipped (Spec C M5 Sub-phase E) and a working tonic-UDS pattern (Spec C M5 D-Sub-E `SomaCustody`). Building khorad in Rust adds zero new vocabulary for the Life operator. Per-host (not per-cluster) keeps the failure blast radius bounded to one host.

**Alternatives rejected.**
- *Go orchestrator* — duplicate operational vocabulary, separate from the rest of life-runtime.
- *Cluster-wide orchestrator* — one cosmic outage instead of N tiny ones; against Life's distributed design.
- *K8s at VM layer* — kube-virt is heavyweight; agent sandboxes don't need pod abstractions; Firecracker's API is the right surface.

### L7-D3: Template store via Lago blob CAS

**Decision.** Templates are stored as `{rootfs.ext4, memfile, snapfile}` triples in Lago's content-addressed blob store (SHA-256 + zstd, already in `crates/lago/lago-store/`). A template is identified by `template_id = sha256(manifest)` where manifest hashes each component. Restoration replays from a known-hash base.

**Rationale.** Life already has content-addressed blob storage at production quality (3000+ tests, SHA-256+zstd, replayable). The provenance chain — "this khora started from template hash X, branched from parent khora Y, ran these N tool calls" — is fully traceable in Lago and joinable to journal events. Multi-region template replication is solved (blob CAS is by-hash; pull on demand).

**Alternatives rejected.**
- *S3/GCS object store* — no provenance integration with Lago events; would require parallel hash-chain infrastructure.
- *OCI registry directly* — OCI manifest format isn't designed for `memfile`/`snapfile`; would need a parallel format anyway.

### L7-D4: ACI surface — five primitives + lifecycle + net policy

**Decision.** The Khora gRPC service (`proto/life/khora/v1/khora.proto`) exposes:

**ACI primitives (5):**
- `Exec(cmd, args, env, stdin, working_dir, timeout) → stream(stdout|stderr|exit)` — bash equivalent; the universal escape valve
- `FsRead(path, byte_range?) → bytes`
- `FsWrite(path, bytes, mode, create_if_missing) → ack`
- `FsEdit(path, hashline_edit) → result` — Blake3 content-addressed line edits (Praxis pattern, `crates/praxis/praxis-tools/src/edit.rs`)
- `FsList(path, pattern, recursive, watch_for) → stream(entries)` — glob + grep + inotify in one verb

**Lifecycle (5):** `Create`, `Pause` (returns snapshot), `Resume` (from snapshot), `Branch` (forks running khora), `Stop`.

**Policy (2):** `UpdateNetPolicy` (hot-swappable, like Vercel Sandbox), `MintEgressToken` (Anima Tier-User cap brokered through lifegw).

**Observability (2):** `Logs` (stream historical + tail), `Metrics` (CPU-ms, RAM-GB-s, FS-bytes, net-bytes, snapshot-GB-h).

**Rationale.** Five ACI primitives match the convergence point of SWE-agent, Computer Use, Devin, Vercel Sandbox, E2B. Adding more (browser, Jupyter, language server) trades agent generalization for surface area — research §3.2 shows the model prefers a small set used in many ways. Anything not in the five runs *inside* via `Exec` (e.g., `Exec("playwright", ["test"])`, `Exec("jupyter", ["nbconvert"])`).

**The hashline `FsEdit`** is a Life-specific tightening — content-hash-addressed line edits give the agent verifiable atomic edits with collision detection, far better than text-substitution diff for long-horizon work.

### L7-D5: Every khora binds an `Arc<dyn AnimaCustody>` at create-time

**Decision.** `CreateRequest` requires `anima_did: String` and `egress_policy: EgressPolicy`. The khora cannot make outbound network calls without minting an egress token via `MintEgressToken`, which requires a Tier-User capability JWS signed by the bound anima identity (Spec D D-Sub-C pattern). Raw API keys never enter the khora; lifegw injects them at the egress proxy edge.

**Rationale.** Spec D shipped the entire capability-token machinery (Tier-1, Tier-2, Tier-User caps; JwksCache; `verify_capability_token`; scope intersection; sub-binding to body.user_id). Khora reuses it. The alternative — sandboxes with raw secrets — has been the source of every credential-leak incident across the comparison products (e.g., Vercel's pre-2026 model required forwarding rules; E2B leaves it to user-injected env vars). Life starts with the secured pattern.

**Alternatives rejected.**
- *Optional anima binding* — invites the "I'll wire it later" anti-pattern; opt-out becomes opt-in only on best behavior.
- *Env-var injection at create-time* — agent code reads secrets; first compromise leaks them.

### L7-D6: `Branch` is first-class; COW rootfs + memfile clone

**Decision.** `Branch(parent_khora_id) → child_khora` produces a running child khora that shares the parent's `rootfs.ext4` (read-only) and a CoW overlay (`/tmp/khora-{id}/upper/`), and clones the parent's `memfile` via `cp --reflink=always` on XFS/Btrfs filesystems. Branch latency target: <50ms. Children diverge cheaply; the parent continues running.

**Rationale.** This is the only primitive in Spec I that no comparison product ships. Vercel and E2B have snapshots; neither has fork-a-running-sandbox. The pattern unlocks **speculative parallel exploration** (research §3.7 notes this as a property the literature *names but doesn't substantiate* — Khora makes it cheap enough to actually use): agent at decision point → `Branch × 4` → run 4 paths to depth N → measure → keep best, Stop rest. Today this is unaffordable; Branch makes it ~$0.04 for a 4-way speculation at 30s per branch on a 4-vCPU class.

**Open question (carries into Sub-F):** memfile clone under concurrent writes. The COW semantics are tested for filesystem (overlayfs is well-understood); memfile concurrency under page-fault during clone is *not* battle-tested at Khora scale. Sub-F conformance battery includes a stress test; if memfile-CoW proves unstable, the fallback is full memfile copy at branch time (~50ms→~4s/GiB) — Branch still works, just less cheaply.

**Alternatives rejected.**
- *Snapshot + create-from-snapshot as branch* — works but ~125ms per child and parent stops while snapshotting; loses the live-fork value.
- *CRIU* — Firecracker's snapshot primitive supersedes; CRIU + Firecracker is not a standard path (research §1.h).

### L7-D7: Every khora action emits `khora.*` event; every second meters Haima

**Decision.** Every Khora RPC produces one `khora.*` event written to Lago via `khora-lago` (sibling of `anima-lago`, `haima-lago`, `arcan-lago`):

| Event | Trigger | Payload |
|---|---|---|
| `khora.created` | Create | template_id, vcpus, ram_mb, anima_did, parent_khora_id?, session_id |
| `khora.paused` / `resumed` / `branched` / `stopped` | lifecycle | khora_id, snapshot_id?, child_khora_id?, reason |
| `khora.exec_started` / `exec_finished` | Exec | cmd, args, exit_code, duration_ms, stdout_bytes, stderr_bytes |
| `khora.fs_*` | FsRead/Write/Edit/List | path, bytes, hashline (for edits), policy_decision |
| `khora.net_egress` | egress through proxy | host, bytes_up, bytes_down, token_id |
| `khora.policy_violation` | denied call | rpc, reason, attempted_resource |

In parallel, **Haima metering events** at 1Hz:

| Meter | Granularity |
|---|---|
| `khora.vcpu_seconds` | per-second active vCPU |
| `khora.ram_gb_seconds` | per-second provisioned RAM-GB |
| `khora.fs_bytes_written` | per-second delta |
| `khora.net_egress_bytes` | per-second delta |
| `khora.snapshot_gb_hours` | per-hour stored |
| `khora.branch_created` | per-branch (one-time) |

**Rationale.** Spec C's pattern is already in place — every substrate emits to Lago, every economic event flows through Haima. Khora is just the next substrate. Full replay via `lago replay --tree` reconstructs any agent session's environmental history. Per-branch billing is what makes speculation an *economic* primitive, not just a technical one — operators can budget speculation explicitly.

### L7-D8: `khora-conformance` battery is the public contract

**Decision.** Ship a `khora-conformance` Cargo crate that validates any `KhoraSession` impl against a fixed test set. `InProcessKhora` (dev), `FirecrackerKhora` (production), `RemoteKhora` (HTTP/gRPC client) all pass the same battery. Third parties can implement Khora-compat backends and prove compliance.

**Rationale.** This is Spec E's E-Sub-F pattern reused. The Khora *protocol* (gRPC + events + metering) becomes the public surface; the *implementation* is swappable. Tenants on `freelance/_template` can run their own khorad and verify against the same battery.

---

## 5. Architecture

### 5.1 Layer model (slots into Life's 8-layer model)

```
L0 — Kernel contract:    aios-protocol::ToolHarnessPort (existing)
L1 — Substrate:          khora-core + khora-envd                             ← NEW
L2 — Adapters:           arcan-khora (kernel→khora bridge)                   ← NEW
L3 — Port traits:        ToolHarnessPort impl, FsPolicyPort, NetPolicyPort
L4 — Tick engine:        arcand calls khora at the body of each tick
L5 — Session orch:       ConsciousnessActor pins one Khora per session
L6 — Transport:          khorad + khora-proxy + lifegw /v1/khora/*           ← NEW
L7 — User:               CLI / chatOS / mission-control                      (existing)
```

### 5.2 Crate layout

```
crates/khora/
├── khora-core/            # Types: KhoraId, Template, Snapshot, EgressPolicy, KhoraEventKind
│                          # Trait: KhoraSession (the ACI)
├── khora-envd/            # Guest daemon (runs inside each VM); gRPC server bound to vsock
├── khora-orchestrator/    # Host-side library; Firecracker VMM lifecycle; UFFD page server
├── khora-template/        # Buildx → rootfs.ext4 → boot → snapshot → Lago blob CAS
├── khora-lago/            # Event sink; KhoraEventKind ↔ EventEnvelope
├── khora-haima/           # Per-second meter → Haima FinancialState
├── khora-vigil/           # OTLP spans for Exec/Fs/Net; Firecracker FIFO metrics
├── khora-anima/           # AnimaCustody binding; capability-token brokerage
├── khora-proxy/           # lifed-side proxy (sibling of arcan-proxy, lago-proxy, anima-proxy)
├── khora-conformance/     # Public test battery; runs against any KhoraSession impl
├── khorad/                # Per-host orchestrator daemon binary
└── khora-cli/             # `life khora create/pause/resume/branch/stop/exec/...`
```

### 5.3 Template pipeline

```
agents/<template>/Khorafile     (Dockerfile-shaped, with Khora-specific verbs)
    │
    ▼ buildx (life khora template build)
OCI image
    │
    ▼ export rootfs (e2fsprogs mkfs.ext4)
rootfs.ext4 (read-only base, ~512MB typical)
    │
    ▼ boot + install khora-envd + run [post-install] hooks
running VM
    │
    ▼ Firecracker CreateSnapshot (memfile + snapfile)
{rootfs.ext4, memfile, snapfile} triple
    │
    ▼ SHA-256 + zstd → Lago blob CAS
template_id = sha256(manifest)
```

A `Khorafile` looks like:

```dockerfile
FROM debian:12-slim
RUN apt-get update && apt-get install -y python3 python3-pip nodejs npm git
RUN pip install --break-system-packages anthropic openai e2b-code-interpreter
COPY --from=khora-envd /usr/local/bin/khora-envd /usr/local/bin/
WORKDIR /workspace
EXPOSE 8000 3000
# Post-install: warm caches, populate /workspace, snapshot-ready signal
RUN python3 -c "import anthropic; import openai"
KHORA_READY
```

`KHORA_READY` is the snapshot trigger — the template builder boots the VM, waits for envd to emit `ready` on vsock, then snapshots. The snapshot captures all of this pre-warmed state.

### 5.4 Guest daemon (khora-envd)

`khora-envd` runs as PID 1 in the VM (init replacement; cleaner than systemd for ~50MB rootfs). Responsibilities:

- gRPC server on vsock port 8000 (host-only addressable; never network-exposed)
- Implements the Khora wire surface from the *inside* (Exec, FsRead/Write/Edit/List)
- Spawns user processes under a non-root `khora-agent` user (UID 1000)
- Pipes process I/O over vsock streams to khorad
- Reports health/heartbeat to khorad every 1s
- Tears itself down on `khorad` saying "stop"

### 5.5 Host orchestrator (khorad)

`khorad` runs as a systemd unit on each Firecracker host. Reuses life-runtime-pool patterns:

- **Warm pool**: pre-spawned VMs for the top-K hot templates, restored via UFFD (sub-100ms create p50 target)
- **Cold path**: full Firecracker boot from `{rootfs.ext4, memfile, snapfile}` (~125ms target)
- **UFFD page server**: per-VM `/tmp/uffd-{khora_id}.sock`, lazy memory restore
- **Egress proxy**: mTLS-terminated proxy in front of every VM's external NIC; enforces `EgressPolicy`; injects bearer-brokered credentials per `MintEgressToken`
- **Saga**: `CreateKhora = MintEgressToken + ProvisionVM + BindAnima + StartEnvd` — same forward+compensate pattern as `lifed` (Spec C M5 Sub-phase E)
- **Pool / breaker**: `life-runtime-pool::Pool<KhoraSession>` per template class, with the same half-open CAS already in life-runtime-pool

### 5.6 Control plane (lifegw + lifed + khora-proxy)

The full Topology B route:

```
HTTPS → lifegw /v1/khora/*
    ├─ TLS 1.3 + JWKS verification (Tier-1 cap for normal ops; Tier-User for cross-tenant operator ops)
    ├─ Rate limit per anima_did + per IP (token bucket; Spec C M7 Sub-phase D)
    ├─ Hot-swappable NetworkPolicy (mirror of UpdateNetworkPolicy semantics)
    └─ WS bearer subprotocol for Exec streaming (Spec C M8.2 pattern)
                │
                ▼ tonic UDS
lifed (saga + pool + breaker)
    ├─ khora-proxy (sibling of arcan-proxy/lago-proxy/haima-proxy/anima-proxy)
    ├─ saga: CreateKhora ≡ {mint_egress_token, provision_vm, bind_anima, start_envd}
    │       compensate: {stop_vm, revoke_token, evict_pool, journal_compensation}
    └─ pool: Pooled<KhoraSession> per template class
                │
                ▼ gRPC over Unix socket (per-host bound)
khorad (per-host orchestrator)
                │
                ▼ vsock
khora-envd (per-VM guest, PID 1)
```

### 5.7 Cross-substrate sinks (the integration map)

Every running khora emits to all five existing substrates in parallel:

| Sink | What flows | Crate |
|---|---|---|
| **Lago** | `khora.*` events (created/paused/exec/fs_*/net_egress/...) | `khora-lago` |
| **Haima** | per-second meters (vcpu_seconds, ram_gb_seconds, ...) | `khora-haima` |
| **Vigil** | OTLP spans (`khora.exec`, `khora.fs_read`, ...), Firecracker FIFO metrics | `khora-vigil` |
| **Anima** | identity binding + egress-token mint via custody-oracle | `khora-anima` |
| **Nous** | tool-call observations (exit_code, duration, stdout_bytes) feed scoring | wire via `khora-lago` event stream |

---

## 6. Surface Contract

Detailed in §5.4 + L7-D4 above. Wire definitions live in `proto/life/khora/v1/khora.proto` (shipped in Sub-A). Trait surface:

```rust
#[async_trait]
pub trait KhoraSession: Send + Sync {
    // Identity
    fn khora_id(&self) -> KhoraId;
    fn anima_did(&self) -> &str;
    fn parent(&self) -> Option<KhoraId>;

    // ACI — 5 primitives
    async fn exec(&self, req: ExecRequest) -> KhoraResult<ExecStream>;
    async fn fs_read(&self, path: &Path, range: Option<ByteRange>) -> KhoraResult<Bytes>;
    async fn fs_write(&self, path: &Path, bytes: Bytes, mode: FsWriteMode) -> KhoraResult<()>;
    async fn fs_edit(&self, path: &Path, edit: HashlineEdit) -> KhoraResult<FsEditResult>;
    async fn fs_list(&self, path: &Path, pattern: Option<Glob>, recursive: bool) -> KhoraResult<Vec<FsEntry>>;

    // Lifecycle — 5 primitives
    async fn pause(&self) -> KhoraResult<Snapshot>;
    async fn resume(snapshot: Snapshot, anima: Arc<dyn AnimaCustody>) -> KhoraResult<Arc<dyn KhoraSession>>;
    async fn branch(&self) -> KhoraResult<Arc<dyn KhoraSession>>;
    async fn stop(&self) -> KhoraResult<()>;

    // Policy
    async fn update_net_policy(&self, policy: EgressPolicy) -> KhoraResult<()>;
    async fn mint_egress_token(&self, request: MintEgressTokenRequest) -> KhoraResult<EgressToken>;

    // Observability
    async fn logs(&self, range: LogRange) -> KhoraResult<LogStream>;
    async fn metrics(&self) -> KhoraResult<KhoraMetrics>;
}
```

---

## 7. Comparison + Differentiation

| Wedge | Vercel Sandbox | E2B | **Khora** |
|---|---|---|---|
| **Substrate** | Firecracker, Amazon Linux 2023 | Firecracker, generic | Firecracker (+ Cloud Hypervisor sibling for GPU) |
| **Cold start (target)** | "milliseconds" (undisclosed) | ~78ms p50 warm, ~150ms cold | <100ms warm-pool p50, <200ms cold; match E2B within 30% by Sub-B |
| **Snapshot/restore** | yes, 30d default expiry | yes, no TTL on paused | yes, Lago-blob backed; **content-addressed** |
| **Branch (fork running)** | ❌ | ❌ | **✅ first-class** (COW + memfile clone) |
| **Identity binding** | OIDC at operator | API key | **✅ AnimaCustody (6 backends), DID-rotated, revocable** |
| **Capability tokens** | egress forwarding rules | env-var injection | **✅ Tier-User caps minted per khora; brokered through lifegw** |
| **Event-sourced** | logs only | OTLP | **✅ `khora.*` events in Lago; `lago replay --tree` reconstructs** |
| **Economic accounting** | active-CPU-ms billing | flat per-create + per-second | **✅ Haima per-meter events; per-branch billable** |
| **Cross-substrate** | siloed | siloed | **✅ Anima + Lago + Haima + Vigil + Nous are siblings, not bolt-ons** |
| **Self-host** | ❌ proprietary | ✅ OSS infra | ✅ Apache-2.0; Life Module pattern |
| **Multi-region** | iad1 only | yes | ✅ content-addressed templates replicate on demand |
| **GPU** | ❌ | partial | ✅ Cloud Hypervisor sibling pool (gated on Spec E inference) |
| **Max session duration** | 45min Hobby / 5h Pro | 5min default / 24h max | unbounded (paused state TTL = ∞; chronos-managed expiry policy) |
| **Inbound port exposure** | 4 max | per-VM URL | not a target use case (agent → outside, rarely outside → agent) |

**The non-wedges (honest accounting):**
- Day-1 cold start will lag E2B's 78ms p50; aim to match within 30% by Sub-B.
- Template ecosystem is empty on day 1; E2B has hundreds. Catalog scale is a year-2 problem.
- Firecracker substrate engineering: we're using it well, not out-engineering it.

**The non-goals:**
- Khora is **not a code interpreter SaaS**. The product story is *agent operating system kernel*, not "run my Jupyter notebook in the cloud."
- Khora is **not a serverless function platform**. Pricing model is per-vCPU-second active + per-snapshot-GB-hour, not per-invocation.
- Khora is **not** a Vercel/E2B competitor at the product layer first; it is the Life substrate first, and the standalone product story emerges if Life's adoption proves the substrate.

---

## 8. Phasing (Khora-Sub-A .. F)

Mirrors Spec D Wave 1–4 pattern and Spec E E-Sub-A..F. Critical-path subphases blocking; parallel subphases marked.

| Sub | Scope | Estimate | Depends on |
|---|---|---|---|
| **Khora-Sub-A** | `khora-core` traits + `khora-envd` skeleton + `InProcessKhora` (no real isolation, parity with `praxis-core::SandboxPolicy`) + ACI primitive contract + conformance battery scaffold + proto definitions | 3–4d | — |
| **Khora-Sub-B** | `khora-orchestrator` Firecracker integration + UFFD lazy restore + `khora-template` Buildx pipeline + warm pool + Lago-blob template store + `FirecrackerKhora` backend passes conformance | 5–7d | Sub-A |
| **Khora-Sub-C** | `khora-proxy` + lifegw `/v1/khora/*` routes + capability tokens + WS bearer for `Exec` streaming + `khora-anima` capability brokerage + soma admin UDS wiring for cross-tenant ops | 4–5d | Sub-A |
| **Khora-Sub-D** | `khora-lago` event sink + `khora-haima` per-second meter + `khora-vigil` OTLP spans + Firecracker FIFO metrics integration + dashboards | 3d | Sub-A (can parallel with B, C) |
| **Khora-Sub-E** | `arcan-khora` adapter → `ToolHarnessPort` impl → Topology B end-to-end live smoke (claude-code-base template + BRO-1013 live-Anthropic smoke now runs INSIDE Khora) + `lago replay --tree` shows khora.* events | 3d | Sub-A, B, C, D |
| **Khora-Sub-F** | `Branch` primitive end-to-end (4-way speculation example agent) + memfile-CoW stress + full `khora-conformance` battery (parity matrix vs E2B + Vercel for ACI primitives) + production deploy guide | 3d | Sub-B, Sub-E |

**Critical path: A → B → C → E ≈ 15 working days** for production-shippable Topology B Khora. Sub-D and Sub-F parallelizable.

**Parallel dispatch (Spec D Wave pattern):**
- Wave 1: Sub-A alone (blocks all others; ~4d)
- Wave 2: Sub-B + Sub-C + Sub-D in parallel (~7d, longest is Sub-B)
- Wave 3: Sub-E (~3d)
- Wave 4: Sub-F (~3d)

Total wall-clock: ~17 working days with parallel dispatch; ~22 days sequential.

---

## 9. Acceptance Criteria

Spec I is "shipped" when:

1. **Conformance**: `cargo test -p khora-conformance` is green against `InProcessKhora` + `FirecrackerKhora` + `RemoteKhora`.
2. **Live smoke**: claude-code-base template + BRO-1013 live-Anthropic smoke (`crates/arcan/arcan-ergon/`) runs inside Khora end-to-end; produces `khora.*` events visible via `lago replay --tree`.
3. **Cold-start p50**: warm-pool restore p50 < 100ms; cold boot p50 < 200ms (measured by `khora-bench` shipped in Sub-B).
4. **Long-horizon**: Pause for 24h, resume on a different host, complete remaining task. Journal shows contiguous events across the pause boundary.
5. **Branch**: 4-way speculation example agent runs to depth 5, keeps best, terminates others. Haima ledger shows 4 distinct branches billed. Lago shows 4 parallel sub-journals.
6. **Cross-substrate**: a single `Exec` call produces (a) Lago `khora.exec_started`/`exec_finished` events, (b) Haima `khora.vcpu_seconds` meter ticks, (c) Vigil OTLP span with proper parent-child to the agent tick span, (d) Anima identity in metadata.
7. **Self-host**: a tenant on `freelance/_template/` runs their own `khorad` + uses Anima custody from their soma + conformance battery is green.
8. **CLAUDE.md "Known gaps" closed**: "No OS-level sandbox isolation" removed; "Network isolation declared but not enforced" removed; "Mount trait defined but unimplemented" addressed (Mount becomes a Khora wire concept).

---

## 10. Open Questions / SPEC-I-DEVIATION Candidates

1. **Memfile CoW concurrency** (L7-D6 footnote). If `cp --reflink=always` + concurrent parent writes produces unstable child memfiles, fallback is full memfile copy (~50ms → ~4s/GiB). Branch still works; latency degrades. Stress test in Sub-F. **Decision deferred to data.**
2. **Cloud Hypervisor GPU pool gating**. Spec E E-Sub-B (`inference-mlx` Apple Silicon) and E-Sub-C (`inference-vllm` CUDA) are still queued. The CH GPU sibling pool should land with E-Sub-C (CUDA path); pulled into Spec I only as a hook. **Deferred to Spec E coordination.**
3. **`Mount` trait** (CLAUDE.md gap). Praxis declares a `Mount` trait but doesn't implement. Khora can subsume it (`FsMount` as an `Exec` of `mount` inside the khora; or first-class `MountVolume` RPC). **Open: first-class or `Exec`-only?** Sub-A decision.
4. **Khora vs Praxis migration path**. Praxis tool registry today; Khora envd inside the VM tomorrow. Sub-E maps the migration but the deprecation timeline for `praxis-tools/src/{fs,shell,edit}.rs` is open. Three options: (a) deprecate praxis-tools, (b) keep praxis-tools as the *dev mode* of khora-envd, (c) keep both and let users choose. **Default in this spec: (b)** — InProcessKhora wraps praxis tools so dev workflows don't break.
5. **Multi-region template replication**. Lago blob CAS is by-hash; pull-on-demand works. Aggressive pre-replication policy is open. **Year-2 problem.**
6. **Inbound port exposure**. Vercel/E2B both expose `*-{id}.<domain>` URLs. Khora's primary use case is agent-driven outbound; do we need an inbound story? **Open; Sub-A decision (currently: no; carved out as Khora-Sub-G later if demand emerges).**
7. **CRIU for cross-host migration**. Firecracker snapshots are tied to a host's CPU features (TSC, AVX, etc.). Cross-host migration requires either feature-pinned host pools or CRIU-mediated migration. **Year-2 problem; deferred.**

---

## 11. References + Linear Ticket Draft

### 11.1 Linear umbrella draft (to file via MCP Linear)

**Title**: `Spec I — Khora Agent Environment Substrate`
**Project**: Life
**Estimate**: 21 pts (3+5+4+3+3+3)
**Priority**: High
**Body**:

> Khora is the agent environment substrate primitive — the Firecracker-backed sandbox that hosts every Life agent. Closes the CLAUDE.md "Known gap" set: no OS-level sandbox isolation, no enforced network policy, no Mount trait impl. Adds first-class snapshot + branch primitives Vercel Sandbox and E2B do not have. Composes with Anima (identity binding), Lago (event sourcing), Haima (per-second metering), Vigil (OTLP spans), and Aegis (policy layer, separate primitive).
>
> Six sub-phases mirroring Spec D Wave 1–4 / Spec E E-Sub-A..F:
> - **Khora-Sub-A** (Urgent, 3pts) — core traits + envd skeleton + InProcessKhora + ACI contract + conformance scaffold + proto
> - **Khora-Sub-B** (High, 5pts) — Firecracker orchestrator + UFFD + template pipeline + warm pool
> - **Khora-Sub-C** (High, 4pts) — proxy + lifegw routes + capability tokens + WS bearer
> - **Khora-Sub-D** (High, 3pts) — Lago + Haima + Vigil sinks
> - **Khora-Sub-E** (High, 3pts) — arcan-khora adapter + Topology B live smoke
> - **Khora-Sub-F** (High, 3pts) — Branch primitive + conformance battery + production deploy guide
>
> Dependencies: B/C/D blocked-by A; E blocked-by A+B+C+D; F blocked-by B+E.
>
> Spec: `core/life/docs/superpowers/specs/2026-05-16-spec-i-khora-agent-environment-substrate.md` (this PR)
> Research substrate: `research/notes/2026-05-15-agent-sandbox-substrates-raw.md`, `research/notes/2026-05-15-aci-thesis-literature-synthesis.md`

### 11.2 Primary references

**Comparison products / research:**
- Vercel Sandbox docs + GA blog (2026-01-30) — Firecracker on Amazon Linux 2023
- E2B `e2b-dev/infra` GitHub — open-source Firecracker + UFFD orchestrator (Go)
- Anthropic Computer Use (Oct 2024) — "general computer skills" thesis
- SWE-agent (Yang et al., arXiv:2405.15793) — Agent-Computer Interface paper
- Cognition Devin launch post — shell + editor + browser primitive set
- `anthropic-experimental/sandbox-runtime` — bubblewrap + sandbox-exec (Claude Code local)

**Life specs (siblings):**
- `docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md` — identity binding + capability tokens pattern
- `docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md` — InferenceBackend (paired primitive)
- `docs/superpowers/specs/2026-04-06-unified-life-filesystem-design.md` — `.life/` filesystem (will add `.life/khora/`)
- Spec C (M5 Sub-phases A–E shipped) — life-runtime cluster patterns Khora reuses

**Life crates (consumed by Spec I):**
- `crates/aios/aios-protocol/src/ports.rs` — `ToolHarnessPort`
- `crates/anima/anima-identity/src/custody.rs` — `AnimaCustody`
- `crates/praxis/praxis-core/`, `praxis-tools/` — soft-sandbox patterns; migration target
- `crates/life-runtime/lifed/`, `lifegw/` — Topology B route Khora joins
- `crates/lago/lago-store/`, `lago-journal/` — template CAS + event sink
- `crates/haima/haima-x402/` — per-second metering
- `crates/vigil/` (planned) — OTLP spans

**Literature on environment-is-the-bottleneck:**
- Hyung Won Chung — *"Add structure for the level of compute you have, then remove it"* (the actually-citable bottleneck quote)
- AgentBench (Liu et al., arXiv:2308.03688) — environment quality bounds agent performance
- TechCrunch 2025-09-21 — Anthropic $1B+ on RL environments
- Anthropic engineering blog "Effective harnesses for long-running agents" (Nov 26 2025)

---

## 12. Status Log

- 2026-05-16 — Spec drafted. Pending: design-decision lock review, Linear umbrella file, optional P20 cross-review before Sub-A.
- 2026-05-16 — Category-C HTML companion authored at `2026-05-16-spec-i-khora-agent-environment-substrate.html` (8 inline-SVG diagrams: naming family, anima/soma trait tree, substrate decision, layer model, template pipeline, branch fork, integration map, phasing waves). Published to broomva docs at `https://broomva.tech/d/khora` (gated, re-versions on iterate). Knowledge graph filed: `research/entities/concept/khora-substrate.md`, `research/entities/pattern/anima-soma-khora.md`. This markdown is the agent-readable canonical (Category A/B); the HTML is the human-read Category-C projection.
