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

- 2026-06-04 — v2 drafted, addresses B1–B10 + S9/S11/S13; S12 acknowledged-and-overridden; S14 (Branch demand) accepted-as-principal's-bet, de-risked via Sub-R.
- 2026-06-04 — **v2 re-gated through P20 (round 2): FAIL at mean 6.0** (Codex 5 · Architect **7 PASS** · Silas 6 · Product 6 — up from v1's 3.5). All reviewers agree the *architecture* is now sound ("removes the live-reflink lie"); held under 7 by specific over-claims, not wrongness. **§9 resolves every convergent round-2 blocker (R1–R13).**

---

## 9. v3 amendments — P20 round-2 response (2026-06-04)

Round 2 confirmed the redesign's architecture but flagged that several fixes were "specified by adjective." Each convergent round-2 finding → a concrete, buildable resolution. (Codex Cn / Architect An / Silas Sn / Product Pn citations.)

### R1 — Branch restore: reflink base + per-child diff, not the diff alone *(C1 blocker, A1)*
A Firecracker diff snapshot contains only pages dirtied since a base — it is **not** restorable alone. Corrected mechanism: Branch reflinks the **full base memfile once** (shared, immutable, resident, refcounted) **+ a per-child diff memfile**; each child's UFFD page server resolves a fault **diff-first-then-base**. If the shared-base + per-child-diff UFFD path proves unstable, the fallback is `snapshot-editor`-merge `base⊕diff → full` per child (the ~seconds/GiB path — explicitly *not* cheap). §2.1 steps 4–5 are corrected accordingly. **Sub-R1 primary gate:** validate N children faulting through one shared base + per-child diff with no base contention.

### R2 — Divergence barrier moves host-side, pre-Resume; agent process is re-exec'd *(C2/C3 blocker, A2 blocker)*
"Block guest until the barrier completes / before any guest userspace resumes" is mechanically false — `restore` resumes vCPUs at the saved instruction pointer, and envd is itself a guest process scheduled *after* resume. Corrected: the barrier runs **host-side, while vCPUs are paused** (`restore(resume=false)`): khorad injects fresh entropy, sets VMGenID, rotates MMDS/identity, clears host-side neighbor/flow tables — **then** `Resume`. Because userspace PRNG state, open fds, and app-protocol state live in guest memory at arbitrary points and **cannot** be sanitized in place (VMGenID reseeds only the *kernel* CSPRNG), Branch's **default is to force-restart the agent runtime process tree (re-exec) on the child after resume** — a fresh process gets fresh userspace PRNG + fds + sockets. Branch is memory-cheap for the *kernel + page cache + warmed libraries*; the agent process re-execs. The alternative — Branch from a **declared-quiescent** parent (no live external sockets; secrets fetched post-genid) — is offered for the rare case needing process continuity. **Sub-R2 gate:** divergence correctness under this model.

### R3 — Egress proxy is a per-khora sidecar, not a shared fleet *(C6 blocker, S1 blocker)*
"Per-tenant trust domain" is now an architecture, not an assertion: **one proxy process per VM** (a sidecar), running in that VM's own host-side netns + cgroup, lifecycle-bound to the khora, holding **only that khora's** brokered credentials, with `kernel.yama.ptrace_scope=3`, coredumps disabled, `mlock` + swap-off. Blast radius = one VM; no shared connection pools across khora. This also fixes the netns topology: `tap → per-khora proxy sidecar socket → egress`. **Sub-G conformance:** khora-A proxy memory is unreachable from khora-B.

### R4 — The host is in the TCB; "never on heap" is retracted *(S2 blocker)*
New invariant statement: **the host is part of the trusted computing base.** The per-tenant memfile-encryption key is HSM/KMS-wrapped, fetched per-restore, used to decrypt into `mlock`ed + `MADV_DONTDUMP` pages, zeroized post-decrypt; **swap is off** on khora hosts. The Vault-transit secret is **transiently** on the sidecar heap (mlocked, zeroized-on-drop — the ChaCha20-Poly1305 pattern already in `haima`). The honest invariant is **"HSM-custodied, minimized-residency, zeroized, mlocked, swap-off"** — the v2 phrase "never on heap" was literally false and is deleted. A host-root compromise yields plaintext; stated plainly.

### R5 — Residual SSRF / tunnel closure for credential-bearing egress *(C5, S3)*
For any credential-injecting destination: **CONNECT / Upgrade / WebSocket are rejected**; one **dedicated connection per token** (no pooling/coalescing across token scopes); **HTTP/2 connection coalescing rejected**; URLs canonicalized; `path_scope` + `method_scope` are a **mandatory deny-by-default exact-path allowlist** (no longer optional); and any credential destination must be vetted **non-forwarding** (no open `url`/`endpoint` params, no server-side fetch) as an operational admission gate. Stated residual: SPKI-pin bounds *where the bytes go*, not *what the trusted endpoint does with them*.

### R6 — Sub-E live-smoke decoupled from Spec E *(P4 blocker)*
AC#2 was unsatisfiable because Sub-E soft-depended on Spec E (17%). Corrected: **Sub-E runs BRO-1013's existing live-Anthropic agent inside `FirecrackerKhora` using the current shipped inference path** (the Anthropic provider) — **not** Spec E's GPU/compute contract. **GPU-exec + the Cloud-Hypervisor sibling pool are scoped OUT of this build** (a separate, Spec-E-gated follow-on). Sub-E is now satisfiable within this build's envelope; the substrate is E2E-validatable without waiting on Spec E.

### R7 — Phasing restated honestly + concrete kill-switches *(P1/P2/P5/P6, C11, A8)*
The serial sum is **12–14 weeks at phase minimums**; applying the conceded first-time-FC/UFFD **×1.4 integration multiplier** ⇒ **planning range 16–24 weeks** (the "11–14" headline is retracted). **Sub-R split:** Sub-R1 (snapshot/UFFD/overlay-reflink + cost curve, 2–3 wk — its FC-orchestrator + UFFD scaffolding is *reused by* Sub-B1/B2, so the weeks are not additive) + Sub-R2 (divergence correctness under R2, 1 wk). **Sub-G split** into review + multi-week fuzzing as an independent gate (not 1 week). **Two concrete kill-switches at the end of Sub-R (before Sub-B2 sunk cost):**
- **wrap-E2B trigger** — if the UFFD page server is not functionally correct within the Sub-R1 timebox **OR** `restore_child_ms` p50 exceeds the threshold set in Sub-R1's plan at 1 GiB → switch to **wrap-E2B**, cancel Sub-B2.
- **Branch kill-switch** — if `branch_N_ms ≥ N × restore_warm_ms` (branching is not cheaper than N cold restores) → **Sub-F descoped to a research follow-on and Branch is cut from the MVP differentiation table.** This makes Sub-R a gate that *can fail the plan*, not theater.

### R8 — khora-pool as an explicit FSM *(C8, A6)*
`khora-pool` is specified as a state machine: `Warming → Idle → CheckedOut → Resetting → Destroyed` (+ `Draining`, `Abandoned`). `reset := destroy + re-restore from template` (**never** in-place — required by §3.7 multi-tenant scrub). Lease expiry + heartbeat liveness; crash recovery via an orphan-VM reaper (the Hawthorne stranded-run-reaper pattern); composite admission predicate `(kvm_slots_free ∧ nvme_warm_budget_ok ∧ cpu_pool_match ∧ tenant_quota_ok)`; exhaustion policy = queue-with-timeout then reject; memfile/overlay refcounts. Conformance cases land in Sub-B2.

### R9 — Warm-pool capacity model + hit-rate SLO *(C9, A4)*
`max_warm_per_host = floor(nvme_budget / uncompressed_memfile_size)`, LRU eviction by `template_id`, replication across CPU-pinned pools factored in. New published SLO: **warm hit-rate**, alongside the two latency SLOs (§5 AC#3). Cold-miss (CAS pull + inflate) is a separate measured slow path with its own SLO. khora-pool admission refuses warm-checkout when the working set exceeds the NVMe budget (explicit cold degrade, not a silent miss).

### R10 — ACI: stable core vs privileged extension tier *(C10, P3)*
Reconciled with the minimal-ACI thesis. **Stable core (MVP, proves the thesis):** `FsRead/Write/Edit/List` + `Exec` + `ProcessControl` (7). **Privileged extension tier (post-MVP, each gated by per-primitive policy + audit events + abuse tests):** `PtyExec`, `StreamFile`, `PortForward`, `MountVolume`, `SetEnv`. The "minimal ACI" claim is retained **for the core only**; extensions are an explicit runtime-management tier, not founding primitives. The differentiation table's "minimal ACI" row is scoped to the core.

### R11 — Cross-host compatibility tuple *(C7, A5)*
L7-D9 sharpened: pool membership = the conjunction `{CPU family+stepping, microcode floor, invariant-TSC freq, host-kernel floor, KVM version, Firecracker version, device-model version}`; resume-admission verifies **every** field and rejects on any mismatch. Cross-host is **same-family, canary-tested** — AWS SnapStart's same-family restriction is **adopted as an invariant**, not cited as an analogy. Default same-host; cross-pool is gated + fail-closed.

### R12 — L2 network hardening + connection-to-VM binding *(S4)*
The tap is **L3-only** (no bridge): `accept_ra=0`, static ARP/NDP, DHCP off (static IP injected by khorad), nftables drops all non-IP ethertypes (ARP/RA/DHCP/link-local). The proxy sidecar binds each connection to its **source VM** (tap/CID) and **rejects any inner token whose `khora_id` ≠ the connection's source VM** — guest self-asserted identity is never trusted.

### R13 — Branch secret-residency: stated invariant + policy *(S5)*
Honest invariant: **Branch inherits all parent process memory by construction; only externally-brokered capabilities (egress tokens) are rotated.** Therefore Branch is **forbidden on VMs flagged secret-bearing** unless either (a) the parent registered a guest-cooperative secret-zeroization callback run pre-snapshot, or (b) the workload fetches secrets only **after** the VMGenID generation change (post-branch). Combined with R2's default process re-exec, the agent re-fetches secrets fresh on each child. "Scrub the credential region without guest cooperation" is retracted as non-functional.

### Net effect on the plan
- Headline phasing: **16–24 weeks** (was an unreachable "11–14").
- Branch latency: **no number stated** anywhere until Sub-R measures it; the differentiation row stays struck until `branch_N_ms < N × restore_warm_ms` is demonstrated.
- Two real kill-switches (wrap-E2B; Branch-cut) now sit at the end of Sub-R, before major sunk cost.
- GPU + Cloud-Hypervisor explicitly out of this build (Spec-E-gated follow-on).

**Pending:** P20 round 3 on the v3-amended spec (final allowed round) before Sub-R kickoff.
