# Spec I v2 — Khora: Agent Environment Substrate (Redesign)

**Date**: 2026-06-04
**Supersedes**: `2026-05-16-spec-i-khora-agent-environment-substrate.md` (v1 — FAILED P20, 2026-06-04)
**Status**: Draft v2 — addresses all 10 P20 substrate/security blockers; principal decision recorded (§0)
**Author**: Carlos Escobar + Claude (Opus 4.8, 1M context)
**Review history**: v1 scored 4/4/3/3 of 10 across Codex/GPT-5 (cross-vendor) + Architect + Silas + product. v2 targets a re-run ≥7.

---

## 0. What changed, and why

v1 failed a 4-way P20 cross-review. The principal's decisions after the verdict:

1. **Build, don't buy.** Khora is built from scratch (not a wrap of E2B), with `e2b-dev/infra` used as a *reference implementation* (UFFD page server, jailer setup, template pipeline) — fork-as-reference, build-as-ours. The honest irreducible reasons (replacing v1's "Rust vocabulary" non-answer) are in §7.
2. **Branch stays a core primitive** — but redesigned to Firecracker's real semantics and gated behind a dedicated research phase (Sub-R) that must produce benchmarks before any economics claim survives.

Every numbered item below maps to a P20 blocker (B1–B10) or strategic finding (S11–S14). The v1 decisions that were *correct* (the cross-substrate fabric thesis, the ACI minimalism instinct, the Lago/Haima/Vigil/Anima sibling integration) carry forward unchanged and are not re-litigated here — read v1 §2–§5 for that framing.

---

## 1. Corrected substrate decisions

### L7-D1 (unchanged) — Firecracker primary; Cloud Hypervisor sibling for GPU; wasmtime inner
Stands. The substrate choice was never the problem.

### L7-D2 (CORRECTED — B8) — new `khora-pool` for VM lifecycle; reuse only the breaker
v1 claimed "reuse life-runtime-pool." **Wrong**: `life-runtime-pool::Pool` is a gRPC *connection* pool (a semaphore over a shared tonic `Channel`) — it models permit-over-a-channel, not stateful exclusively-owned bootable resources. Khora needs a **new `khora-pool`**: warm-set keyed by `template_id`, exclusive checkout, post-use reset-or-destroy, host-capacity admission control, host affinity. The existing `CircuitBreaker` is reused **only** for the khorad↔envd gRPC link, not for VM capacity (an "out of KVM slots" condition is not a gRPC error rate).

### L7-D3 (CORRECTED — B6) — split the template store: rootfs in CAS, memfile local + uncompressed
v1 put the whole `{rootfs.ext4, memfile, snapfile}` triple in Lago zstd CAS. **This defeats UFFD**: you cannot lazily page-fault out of a compressed blob — you must inflate the entire GB-scale memfile to a local path first, which is seconds and erases the warm-pool target. Corrected model:

| Artifact | Storage | Why |
|---|---|---|
| `rootfs.ext4` (immutable base) | Lago CAS (zstd, SHA-256) | pulled once per host, cached; provenance by hash; zstd fine (block device, not page-faulted) |
| `memfile` (memory image) | **local NVMe, uncompressed** | UFFD page-faults pages out of it live; must be resident + uncompressed for the lifetime of the resumed VM |
| `snapfile` (VMM state) | local + CAS archive | tiny; both |
| cold archive of a full template | Lago CAS | provenance + cross-host seeding; inflated to local NVMe on first use, never the hot restore source |

The warm-pool latency SLO applies **only** to host-resident (already-inflated) templates. A cold CAS pull is explicitly a slow path and is measured separately.

### L7-D4 (EXPANDED — B9) — the ACI surface grows from 5 to a sufficient set
v1's five primitives (`Exec`, `FsRead/Write/Edit/List`) cannot actually manage a real agent runtime. v2 surface:

**Filesystem (4):** `FsRead`, `FsWrite`, `FsEdit` (Blake3 hashline), `FsList` (glob+grep+watch).
**Process (3 — NEW):** `Exec` (one-shot), `ProcessControl` (signal / kill / list / wait — long-running + background processes), `PtyExec` (interactive TTY + resize — REPLs, `vim`, anything that probes `isatty`).
**Transfer (2 — NEW):** `StreamFile` (large-file up/down without buffering in a gRPC message), `PortForward` (host-mediated inbound when an agent serves a dev server — was carved out of v1, now first-class behind policy).
**Mount (1 — NEW, B-Architect):** `MountVolume` — **host-mediated** (khorad attaches a virtio-blk drive or pre-mounts before handoff). Never an in-guest `Exec("mount")` — that needs `CAP_SYS_ADMIN` the untrusted agent user (UID 1000) must not hold.
**Env (1 — NEW):** `SetEnv` — structured secret/env injection via vsock control channel, not leaked through `Exec` argv/environ.

**PID 1 (B9 / Architect-10):** the guest init is a minimal reaper (`tini`/`catatonit`), with `khora-envd` a *supervised child*. Zombie reaping, orphan re-parenting, signal forwarding to the process group, and mounting `/proc`,`/sys`,`/dev/pts` are explicit envd-bootstrap responsibilities — not hand-waved as "cleaner than systemd."

Everything still not in this set (browsers, Jupyter, language servers) runs *inside* via `Exec`/`PtyExec`. The set grew because runtime *management* (kill a hung process, resize a PTY, serve a port, stream a 2GB artifact, attach a dataset) is not optional — but it is still small and human-computer-shaped, not a task-API zoo.

### L7-D5 (REWRITTEN — B3,B4,B5) — the egress + identity security model
This was the most-broken decision. Full rewrite in §3.

### L7-D6 (REWRITTEN — B1,B2) — Branch, corrected to real Firecracker semantics
Full rewrite in §2.

### L7-D7 (unchanged + HARDENED — B5) — events + metering + at-rest encryption
Event sourcing and Haima metering stand. **Added**: memfile/snapfile encrypted at rest (§3.4).

### L7-D8 (unchanged) — `khora-conformance` is the public contract
Stands. Expanded to cover the new ACI primitives + the security invariants as conformance tests (default-deny egress must fail-closed; a recycled VM must be scrubbed; a branch child must not inherit a live token).

### L7-D9 (NEW — B7) — cross-host resume requires CPU-feature-pinned host pools
v1's acceptance criterion #4 ("resume on a different host") contradicted its own open question and is unsafe (Firecracker snapshots are pinned to host TSC frequency + CPUID feature mask). **Decision:** cross-host resume is supported **only within a CPU-template-normalized host pool** (fixed TSC, normalized CPUID à la AWS Lambda SnapStart). The khorad scheduler enforces pool affinity on resume. Resume to an incompatible host is rejected, not attempted. Default acceptance is **same-host** resume; cross-pool resume is the explicit, gated capability.

### L7-D10 (NEW — S13) — Khora is the one isolation primitive; Aegis is its policy
v1 created three competing sandbox layers (praxis soft-sandbox, planned Aegis, Khora). **Decision:**
- **Khora** is the *only* OS-level isolation substrate. The praxis soft-sandbox becomes `InProcessKhora` — the dev-mode `KhoraSession` impl (no microVM), used for local/`arcan serve` runs. There is exactly one isolation interface (`KhoraSession`); praxis-tools live behind it.
- **Aegis** is redefined from "a second sandbox" to **the policy layer that configures Khora**: it owns the *content* of `EgressPolicy`, capability scopes, attestation, and secret-brokering *rules* — but enforcement lives in Khora's substrate (default-deny netns, the egress proxy, the capability verifier). Aegis decides *what is allowed*; Khora *is the box*. No overlapping isolation code.

This resolves S13 explicitly and is recorded as a cross-spec amendment to the Aegis roadmap entry.

---

## 2. Branch — corrected (B1, B2) · kept as a core primitive

The principal kept Branch as the marquee differentiator. It is preserved — but the v1 mechanism ("`cp --reflink` a live memfile, parent keeps running, <50ms") is **mechanically impossible on Firecracker** and is replaced.

### 2.1 The real mechanism

Firecracker exposes exactly one snapshot path: `Pause → CreateSnapshot → (resume | restore)`. Loaded memory files must be immutable. So Branch is:

```
1. khorad: Pause(parent)                         # parent halts — single-digit→low-tens of ms
2. khorad: CreateSnapshot(parent, type=Diff)     # diff snapshot = only dirty pages since base; cheap
3. khorad: Resume(parent)                         # parent runs again; total pause window is the snapshot cost
4. khorad: reflink the now-IMMUTABLE snapshot memfile (cp --reflink) — O(metadata), instant
5. khorad: for each child i in 1..N:
     restore(child_i) from {reflinked memfile, snapfile}   # UFFD lazy restore, ~tens of ms each
     run the DIVERGENCE HOOK (§2.2) before unfreezing guest code
```

**Honest framing:** the parent is *briefly paused* (the snapshot window), not "continuously running through a live fork." Children are *restores from a frozen snapshot*, not live clones. Diff snapshots keep the pause window small (only dirty pages serialized).

### 2.2 The divergence hook (B2 — mandatory, was entirely missing)

N children booted from one memory image share state that **corrupts them** unless reset. Firecracker provides the **VMGenID** device precisely for clone detection — the guest kernel sees a generation-ID change and can trigger reseed. `khora-envd` runs a mandatory barrier *before* any guest userspace code resumes:

| Hazard | Divergence-hook action |
|---|---|
| Shared CSPRNG / `/dev/urandom` state → key/nonce collisions | VMGenID-triggered kernel reseed + fresh entropy injected via vsock (`RNDADDENTROPY`) |
| Identical TCP seq / live ESTABLISHED sockets → RST / cross-talk | tear down all inherited sockets; agent must re-open |
| Duplicated MMDS / instance identity | rotate `khora_id`, MMDS token, `boot_id` |
| Frozen TSC / `CLOCK_MONOTONIC` jump | clock-jump notification (Firecracker clock feature) |
| **Inherited live egress token (B5)** | **invalidate parent's token; re-mint a fresh child-scoped one** — never inherit |
| Duplicated open fds | close inherited external fds; workspace fs is the CoW overlay (safe) |

Branch **refuses** if the parent holds open external sockets it cannot safely tear down (configurable: refuse vs force-close). Guest code is blocked until the barrier completes. This barrier cost is part of the Branch latency budget and is measured in Sub-R.

### 2.3 Sub-R — the dedicated Branch research phase (the principal's condition)

No Branch economics claim survives into the spec until **Sub-R** produces data. Sub-R deliverables:

- Benchmark `pause_ms`, `snapshot_create_ms` (full vs diff), `reflink_ms`, `restore_child_ms`, `divergence_hook_ms`, and end-to-end `branch_N_ms` at memfile sizes **512 MiB / 1 GiB / 4 GiB**.
- Validate diff-snapshot reflink correctness (children boot, diverge, no corruption) under a stress harness (concurrent parent writes during the pause window; entropy/socket/TSC checks post-hook).
- Produce the **real** cost curve and decide the honest economic claim (or retract "speculation is cheap" if the numbers don't support it).
- Decide the fallback policy if diff-snapshot reflink proves unstable (full memfile copy ~seconds/GiB — explicitly *not* cheap).

Sub-R **gates** Sub-F (the production Branch primitive). The "<50ms" number is deleted from the spec until Sub-R measures the truth.

---

## 3. Egress + identity security model (B3, B4, B5 — full rewrite)

v1's `MintEgressToken` was a confused deputy with a contradictory TLS model and no enforcement. Rewritten.

### 3.1 Default-deny network namespace (B4 — enforcement, was absent)

The VM's tap device is **default-drop** at the host kernel (nftables/conntrack on the host side of the tap). The **only** route off-VM is the egress-proxy socket. A raw `connect()` to an arbitrary IP from inside the guest **fails closed**. This is a `khora-conformance` test: `Exec("curl https://1.1.1.1")` with no matching policy must fail at the kernel, not at an app-layer check. No raw TCP/UDP/ICMP/QUIC/DNS escapes the netns.

### 3.2 Destination-bound capability tokens (B3 — confused deputy fix)

The egress capability is bound to a **canonical destination**, not just `user_id`+`scope`. Token claims:

```
{ khora_id, branch_id, dst_host, dst_port, dst_spki_pin, method_scope,
  path_scope, nonce, exp, byte_quota, req_quota }
```

Enforcement:
- **Resolve-once, connect-to-resolved.** The proxy resolves `dst_host` at mint time, pins the IP, and connects only to that IP — defeating DNS rebinding.
- **Cert SPKI pin.** The credential is injected only if the destination's cert matches `dst_spki_pin` — defeating lookalike/redirect hosts.
- **No wildcards for credential-bearing egress.** `*.openai.com` is allowed for *plain* egress but never for a token that injects a real key.
- **Redirects rejected** on credential-bearing requests.
- **Replay cache** keyed by `nonce`; tokens are single-flight within `exp`.

### 3.3 The proxy trust boundary (B3 — TLS contradiction resolved)

Decided explicitly: the egress proxy is a **TLS-terminating forward proxy operating in the tenant's trust domain** (per-tenant, not a shared multi-tenant MITM). It terminates TLS to inject the credential, so it necessarily sees plaintext for that tenant's brokered destinations — that is the accepted, scoped trust boundary, and the injected secrets live in **Vault transit / an HSM**, never on the proxy heap. SNI-passthrough mode exists but **cannot inject headers** (stated plainly) — passthrough destinations must carry their own auth. The proxy operator, key custody, and audit scope are named in the deploy guide.

### 3.4 Snapshot secrecy (B5)

- memfile/snapfile **encrypted at rest** with a per-tenant key that is **never present in the snapshot**.
- Lago CAS enforces **per-tenant read-ACL** — knowing a blob hash is *not* authorization (corrects v1's "by-hash, broadly readable").
- On `Branch`/`Resume`, egress tokens are **re-minted, never inherited** (§2.2).
- Best-effort credential-region scrub before snapshot where the guest cooperates.

### 3.5 AnimaCustody — runtime authorization, not just create-time binding (B3-Silas)

v1 bound `Arc<dyn AnimaCustody>` at create and never consulted it again. Added: an explicit `authorize_egress(khora_id, dst) → EgressGrant` checked on **every mint and every connection**, with a **live revocation lookup** (not a cached snapshot — corrects the stale-handle-after-`rotate()` hole) and a TTL forcing re-binding across pause/resume/branch.

### 3.6 vsock as an untrusted boundary (B-Silas-6)

`khora-envd` (runs untrusted guest code) → `khorad` (host orchestrator) over vsock is an **untrusted** channel. Per-VM message-size cap + request-rate limit; khorad treats all guest input as hostile; the khorad gRPC server is **fuzzed against hostile guest input as a Sub-B gate**. A compromised guest cannot exhaust khorad memory/fds or starve sibling VMs.

### 3.7 Multi-tenant scrub + UpdateNetPolicy teardown (B-Silas-7,8)

- A pooled VM reassigned across `anima_did` is **fully reset** (fresh memfile, fresh overlay) — or pooled VMs are single-tenant-templated and never carry tenant data. Conformance-tested.
- Per-tenant CPU/RAM/IO quotas so the per-second meter cannot be weaponized for noisy-neighbor DoS.
- `UpdateNetPolicy` tightening **tears down all now-disallowed flows** (drop, not merely stop-new).

---

## 4. Re-baselined phasing (B10 — was 3–5× optimistic)

v1's "15 working days" for a from-scratch Firecracker substrate was not credible. Honest plan, **serial** (solo/small-team — no fictional parallel waves), with research and security as their own phases:

| Phase | Scope | Estimate |
|---|---|---|
| **Sub-R** | Branch + UFFD + cross-host-pinning research spike — benchmarks, diff-snapshot reflink validation, divergence-hook prototype, the honest cost curve | 1–2 wk |
| **Sub-A** | `khora-core` traits + envd skeleton + `InProcessKhora` + **expanded** ACI contract + conformance scaffold + proto | 1 wk |
| **Sub-B1** | FC orchestrator + snapshot-restore **without UFFD** (full memfile load) + template pipeline (rootfs CAS + memfile-local) + jailer/cgroups-v2 | 2 wk |
| **Sub-B2** | UFFD lazy-restore page server (`UFFDIO_COPY`, partial faults, residency lifetime) + warm pool + `khora-pool` | 2 wk |
| **Sub-C** | egress proxy (TLS-term, destination-bound tokens, default-deny netns) + lifegw routes + capability brokerage + AnimaCustody egress authz | 2 wk |
| **Sub-D** | Lago/Haima/Vigil sinks + snapshot at-rest encryption + CAS per-tenant ACL | 1 wk |
| **Sub-E** | `arcan-khora` adapter + Topology B live smoke inside Khora | 1 wk |
| **Sub-F** | Branch primitive (pause→snapshot→restore + divergence hook) — **gated on Sub-R + Sub-B2** | 1–2 wk |
| **Sub-G** | dedicated security review + fuzzing (vsock, proxy, multi-tenant scrub, default-deny) + conformance security tests | 1 wk |

**Realistic critical path ≈ 11–14 weeks**, not 15 days. Sub-B is split (B1 ships value without UFFD; B2 adds the fast path) so there is a working substrate before the hardest component. Sub-R gates the Branch economics; Sub-G gates production.

---

## 5. Corrected acceptance criteria (B7, S9)

1. `cargo test -p khora-conformance` green across `InProcessKhora` + `FirecrackerKhora` + `RemoteKhora`, **including** the security invariants (default-deny egress fails closed; recycled VM scrubbed; branch child does not inherit a live token).
2. Live agent (BRO-1013 smoke) runs inside a `FirecrackerKhora`; `khora.*` events in `lago replay --tree`.
3. Two SLOs published from `khora-bench`: **restore-to-runnable** p50 and **restore-to-warm** p50 (working set faulted) — the warm-pool target commits to the latter.
4. **Same-host** 24h pause→resume completes; **cross-pool** resume works only within a CPU-template-normalized host pool (L7-D9), and is *rejected* (not attempted) outside it.
5. Branch: Sub-R benchmarks published; a 4-way branch runs to depth 5, divergence hook verified (no entropy/socket/identity collision), Haima bills 4 branches, Lago shows 4 sub-journals — **with the economic claim stated only against measured numbers**.
6. One `Exec` → Lago events + Haima meter + Vigil span (tick-parented) + Anima metadata.
7. Self-host: a `freelance/_template` tenant runs `khorad` + soma-backed custody; security conformance green.
8. CLAUDE.md gaps closed **for production Khora-hosted sessions** (stated honestly — the `InProcessKhora` dev-mode soft path persists for `arcan serve`, corrects v1's over-claim S9).

---

## 6. What still carries from v1 (unchanged, was correct)

- §2 vision (the receptacle / anima·khora pairing), §3 the ACI thesis grounding, the cross-substrate fabric (Lago/Haima/Vigil/Anima/Nous as siblings), the Plato framing, the differentiation-vs-Vercel/E2B table (minus the now-corrected Branch + cold-start rows), the economic-model meters. Read v1 for these.

---

## 7. Build-vs-buy — the honest answer (S11)

The principal chose build. The honest irreducible reasons (replacing v1's "Rust vocabulary"):

1. **Branch needs to live in the VMM control path.** The divergence hook + diff-snapshot-reflink + pause-window orchestration are operations E2B's control plane does not expose; owning khorad is owning that path.
2. **Per-action custody + event sourcing at the orchestrator.** Binding `AnimaCustody` and emitting `khora.*` per RPC is deepest at the orchestrator, not bolted onto a foreign Go control plane.
3. **One-language operational surface** (pool/breaker/saga/OTLP already in Rust) — a real maintenance reason, but explicitly *secondary*, not the headline.
4. **The substrate is the long-term product.**

**Honesty clause (kept from the review):** `e2b-dev/infra` (Apache-2.0) is the *reference* for the UFFD page server, template pipeline, and jailer setup — we read it, we don't rewrite blindly. And the **wrap-E2B fallback stays live**: if Sub-R or Sub-B2 prove the UFFD page server is disproportionately costly to build correctly, wrapping E2B's orchestrator (and emitting the cross-substrate events over it) is the documented retreat — the cross-substrate fabric does not *require* our own khorad, it requires event emission at the orchestration layer.

**Sequencing (S12) — acknowledged, principal-overridden:** Spec E (the paired compute contract) is at 17% and M9 apps/chat is the stated critical path. Building Khora now is a deliberate choice to prioritize the substrate over those; Sub-E (live smoke) and the GPU sibling pool still soft-depend on Spec E reaching ≥E-Sub-C, so those two items remain sequenced behind Spec E even within this build.

---

## 8. Status

- 2026-06-04 — v2 drafted, addresses B1–B10 + S9/S11/S13; S12 acknowledged-and-overridden; S14 (Branch demand) accepted-as-principal's-bet, de-risked via Sub-R. Pending: re-run P20 on v2 before Sub-R kickoff.
