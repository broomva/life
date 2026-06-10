# ADR — Anima signing surface for ergon attestation

- **Status**: Proposed
- **Date**: 2026-05-22
- **Linear**: [BRO-1226](https://linear.app/broomva/issue/BRO-1226/ergon-phase-2-gap-2c-anima-adapter-design-agentsoul-signing-surface)
- **Parent**: [BRO-994](https://linear.app/broomva/issue/BRO-994) (Ergon v0.1 umbrella, Done)
- **Sibling**: [BRO-1225](https://linear.app/broomva/issue/BRO-1225) (Nous adapter design — same pattern, paper-only, sibling crate)
- **Downstream consumer**: [BRO-1217](https://linear.app/broomva/issue/BRO-1217) (M9-G AAP verifier, Done)
- **Scope**: paper-only design + minimal trait skeleton; implementation lands in a follow-up

## Context

Ergon's attestation hook lives at `crates/ergon/ergon-life-hooks/src/attestation.rs`. The existing trait is:

```rust
#[async_trait]
pub trait SoulAttester: Send + Sync {
    async fn sign_session_start(&self, session_id: &SessionId, workflow_name: &str)
        -> std::result::Result<(), String>;
    async fn sign_session_end(&self, session_id: &SessionId, workflow_name: &str, ok: bool)
        -> std::result::Result<(), String>;
}
```

`AnimaAttestHook` calls this on `on_workflow_start` and `on_workflow_end`. The hook is **session-boundary only** — it signs the start/end of a workflow run. **Step receipts (one per inference step inside the workflow) are NOT yet attested.** That is the scope BRO-1226 opens.

> **Correction to the ticket's framing**: BRO-1226 talks about an existing "`Signer` trait that doesn't have a real impl." The actual existing trait is `SoulAttester`, and the gap is two-fold: (a) wire the existing session-boundary `SoulAttester` to real Anima crypto; (b) extend it to per-step receipts (a new shape, not covered by the existing trait). This ADR addresses both — the new trait `AgentAttestationSigner` covers per-step receipts; the existing `SoulAttester` gets a real impl via the same adapter.

Anima already exposes substrate-grade signing:

```rust
// crates/anima/anima-identity/src/custody.rs
pub trait AnimaCustody: Send + Sync + 'static {
    fn user_did(&self) -> &str;
    fn auth_pubkey(&self) -> [u8; 33];
    fn sign_jws(&self, claims: &Value) -> AnimaResult<String>;
    fn sign_digest(&self, digest: &[u8; 32]) -> AnimaResult<[u8; 64]>;
    fn sign_eip712(&self, domain: &Eip712Domain, types: &Value, message: &Value)
        -> AnimaResult<EvmSignature>;
    fn rotate(&self) -> AnimaResult<(DidRotationEvent, Arc<dyn AnimaCustody>)>;
    // ...
}
```

There are 6 production backends behind `AnimaCustody` (Vault, softhsm, WebCrypto, RemoteAnima, HardwareWalletAnima, VaultTransitAnima). The signing surface is solid. What's missing is the **adapter** that maps ergon attestation events onto `AnimaCustody::sign_jws`.

## Decisions

### 1. Trait location

**Decision**: a new crate `crates/ergon/ergon-anima-adapter/`. The trait `AgentAttestationSigner` lives in its `src/lib.rs`. The crate also ships the production impl of the existing `ergon_life_hooks::SoulAttester` trait — so this one crate is the adapter for both session-boundary attestation and per-step receipts.

**Justification**: same decoupling argument as BRO-1225's `ergon-nous-adapter` (sibling crate). `anima-identity` stays a generic custody primitive with no ergon awareness. `arcan-ergon` is the workflow runner, not the right home for cross-substrate adapter contracts. A dedicated adapter crate keeps the dep direction one-way (`ergon-anima-adapter → ergon + ergon-life-hooks + anima-identity`) and matches the pattern Phase 2 is establishing.

Spec C₃ §11.2 dependency rules confirmed: anima is leaf; arcan-ergon depends on anima; ergon-core stays substrate-agnostic. The new crate respects this — it depends on both anima-identity and ergon-life-hooks but is itself a leaf with respect to the workflow runtime.

### 2. Custody-backend abstraction

**Decision**: the adapter takes `Arc<dyn AnimaCustody>` at construction time. The ergon-side trait does **not** parameterize over backend type. Per-call, the adapter dispatches through `custody.sign_jws(claims)` (or `sign_digest` for raw-binary use cases). The 6 production backends remain entirely behind `AnimaCustody` — the adapter neither knows nor cares whether `sign_jws` ends up calling into Vault, softhsm, WebCrypto, or a future HSM.

```rust
pub struct AgentAttestationAdapter {
    custody: Arc<dyn AnimaCustody>,
    journal: Arc<dyn AttestationJournal>,  // emits signed receipts onto lago
}
```

**Justification**: `AnimaCustody` is the existing custody abstraction (`crates/anima/anima-identity/src/custody.rs:202`). Re-parameterizing in the adapter would either fork the abstraction or leak backend types upward. `Arc<dyn AnimaCustody>` is the right level — same as how the rest of the codebase passes custody around (e.g., `lifegw::auth::kms::KmsSigner` user-scope analog).

The adapter constructor takes `Arc<dyn AnimaCustody>` directly. The caller (the arcan workflow runner — BRO-1001 wiring) resolves the right backend at startup and passes it in. The adapter has no resolver.

### 3. Signature shape

**Decision**: **JWS (ES256, kid=DID, typ=agent+receipt+jwt)**. Matches the existing AAP-verifier shape (`apps/broomva/lib/lago-auth/verify-jwt.ts` from M9-G / BRO-1217) with `typ` distinguishing receipt-JWTs from auth-JWTs.

```jsonc
// Header
{
  "alg": "ES256",
  "typ": "agent+receipt+jwt",
  "kid": "did:key:zDn..."
}
// Claims
{
  "iss": "<jwk-thumbprint>",       // SHA-256 of canonical JWK
  "sub": "<agent-id>",
  "aud": "lifegw",
  "iat": 1716422400,
  "jti": "<uuidv4>",
  "session": "<session_id>",
  "workflow": "<workflow_name>",
  "step": 7,
  "output_sha256": "<hex>",
  "parent_session": "<session_id>"   // optional
}
```

**Justification**:

- **JWS over bare ECDSA**: bare 64-byte ECDSA loses the binding to a verifying key. The verifier would need out-of-band metadata to know which DID's pubkey to check. JWS encodes `kid=DID` in the header — self-contained verification.
- **JWS over COSE**: COSE is binary-CBOR and would need a separate codec on the broomva.tech side. The M9-G verifier already speaks JWS. Reusing one codec is the simpler operations story.
- **ES256 (P-256), not secp256k1**: matches the existing AAP verifier (Spec C₃). secp256k1 is for wallet flows (haima/x402), not for identity attestation.
- **`typ=agent+receipt+jwt`**: distinguishes from `typ=agent+jwt` (M9-G AAP auth tokens) so the verifier can route by `typ`. The auth-JWT shape stays unchanged; only `typ` differs in the receipt shape. M9-G AAP verifier changes required: minimal (one branch on `typ`).

### 4. What gets signed

**Decision**: a **canonical-JSON receipt** built from these fields, in this order:

| Field | Source | Notes |
|---|---|---|
| `session` | `HookCtx::session_id` | Required |
| `workflow` | `HookCtx::workflow_name` | Required |
| `step` | `HookCtx::step_index` | u32 (0-based) |
| `agent_did` | `custody.user_did()` | Required; mirrors `kid` in JWS header |
| `iat` | `SystemTime::now()` | UTC seconds |
| `output_sha256` | SHA-256 of canonical-JSON-serialized `ModelResponse.content` | Required |
| `tool_calls` | array of `{tool, sha256(args), sha256(result), ok}` | One entry per tool invocation in the step |
| `parent_session` | `HookCtx::parent_session_id` if present | Optional |

The receipt is serialized with **canonical JSON** (sorted keys, no whitespace, UTF-8). The serialized bytes are then handed to `custody.sign_jws(receipt)`.

**Justification**:

- **Hash, not full content**: receipts must be small enough to ship inline with events and small enough to store millions per session. The output bytes themselves go to lago (already content-addressed there); the receipt just carries the hash.
- **Per-tool fingerprints**: enables provenance audits (was this tool result altered between emission and consumption?). `sha256(args)` + `sha256(result)` keep the receipt size bounded regardless of payload size.
- **Canonical JSON, not EIP-712**: EIP-712 is for wallet flows (haima/x402 USDC), where the verifier is an Ethereum contract. Identity attestation flows through lifegw — JWS/canonical JSON is the established lifegw codec.
- **Avoid a separate domain-separator**: `typ=agent+receipt+jwt` in the JWS header already disambiguates from `agent+jwt`. Adding `kind: "receipt"` to the body would be redundant.

### 5. Verification path

**Decision**: **lifegw on receive**, using the existing AAP verifier surface at `apps/broomva/lib/lago-auth/verify-jwt.ts`. The verifier already does multi-curve rotation-aware ES256 verification (M9-G). Receipt JWTs branch on `typ=agent+receipt+jwt` and go through a thin extension of the existing path.

**Verification flow** (lifegw side):

1. Parse JWS header → extract `kid` (DID) and `typ`
2. If `typ != agent+receipt+jwt` → reject (out of scope for receipt verifier)
3. Resolve DID → pubkey via existing journal resolver + rotation-chain walk
4. Verify ES256 signature over `header.body` bytes
5. Decode claims → validate `iat` against `lifegw_clock_skew_max_sec` (60s default)
6. Audit-log + emit `gen_ai.attestation.verified` OTel span event
7. Return decoded claims to the calling route

**Out of scope**: soma admin plane verification (downstream — once a receipt is verified at lifegw, downstream services trust the lifegw-stamped headers). A separate `ergon-verifier` service is unnecessary — lifegw is the single trust boundary.

**Justification**: doubling up on verifier infrastructure (soma + lifegw + ergon-verifier) creates trust-boundary ambiguity and triples the maintenance surface. The pattern that works for AAP auth tokens — single lifegw verifier, downstream services trust the gateway — is the right shape here too. The M9-G verifier already handles rotation-chain walking; adding a `typ=agent+receipt+jwt` branch is a small extension, not a new surface.

Possible follow-up: a thin Rust-side verifier in `crates/lago/lago-auth/` for in-process receipt verification (used by lago itself when re-ingesting events). Defer to implementation review.

### 6. Key-rotation interaction

**Decision**: **rotation-chain walk on verify** — receipts signed under an old DID stay verifiable forever. No re-signing of historical receipts. No hard cut-over.

When the agent rotates (via `AnimaCustody::rotate()`):

- The new `Arc<dyn AnimaCustody>` is swapped in by the runner; future receipts use the new DID.
- Existing in-flight receipts (signed under the old DID) keep their original `kid=did:key:<old>` header.
- The verifier resolves `kid` → walks the journal's rotation chain (`anima_lago::rotation_events`) → finds the cryptographic continuity → accepts the signature against the OLD pubkey.

**Justification**:

- Re-signing historical receipts is operationally impractical: receipts may be millions per session × thousands of agents. Re-signing on every rotation creates a write storm.
- A hard cut-over (receipts signed before rotation become invalid) violates audit semantics — past actions don't become un-attested just because the agent rotated its key.
- Rotation-chain walking is **already implemented** in the M9-G verifier (`apps/broomva/lib/lago-auth/rotation-chain.ts`). The receipt verifier reuses the same `DidRotation[]` resolver + walk.
- Spec D L4-D10 already commits to "the rotation event carries a proof JWS signed by the OLD key over the NEW key." That chain is the cryptographic substrate for receipt verification across rotations.

The cost is upfront: the rotation-chain resolver must be available at verification time. This is acceptable because (a) journal resolvers are mandatory for AAP verification already; (b) lifegw caches resolved chains; (c) the journal is the source of truth — there is no separate state to keep consistent.

## Skeleton trait

Committed at `crates/ergon/ergon-anima-adapter/src/lib.rs` in this PR:

```rust
//! Anima-backed implementation of ergon attestation traits.

use std::sync::Arc;
use anima_identity::AnimaCustody;
use async_trait::async_trait;
use ergon::SessionId;
use ergon_life_hooks::SoulAttester;
use serde_json::Value;

/// Per-step attestation signer. New trait — covers step-receipts the
/// existing SoulAttester (session-boundary only) doesn't reach.
#[async_trait]
pub trait AgentAttestationSigner: Send + Sync {
    async fn sign_step_receipt(
        &self,
        receipt: &Value,  // canonical-JSON receipt body — see ADR §4
    ) -> Result<String, String>;  // returns compact JWS
}

pub struct AgentAttestationAdapter {
    custody: Arc<dyn AnimaCustody>,
}

impl AgentAttestationAdapter {
    pub fn new(custody: Arc<dyn AnimaCustody>) -> Self {
        Self { custody }
    }
    pub fn agent_did(&self) -> &str {
        self.custody.user_did()
    }
}

#[async_trait]
impl AgentAttestationSigner for AgentAttestationAdapter {
    async fn sign_step_receipt(&self, _receipt: &Value) -> Result<String, String> {
        Err("AgentAttestationAdapter::sign_step_receipt not yet implemented; see ADR §3".into())
    }
}

#[async_trait]
impl SoulAttester for AgentAttestationAdapter {
    async fn sign_session_start(&self, _session_id: &SessionId, _workflow_name: &str)
        -> Result<(), String> {
        Err("sign_session_start not yet implemented; see ADR §4".into())
    }
    async fn sign_session_end(&self, _session_id: &SessionId, _workflow_name: &str, _ok: bool)
        -> Result<(), String> {
        Err("sign_session_end not yet implemented; see ADR §4".into())
    }
}
```

This compiles against the existing `ergon`, `ergon-life-hooks`, and `anima-identity` crates without modifying any of them. The implementation follow-up will:

- Build canonical-JSON receipts per §4.
- Wire `custody.sign_jws(receipt)` → JWS string.
- Emit the signed JWS onto the lago journal (via an injected `AttestationJournal` trait — to be designed in the implementation ticket).
- Add the four failure-mode branches: backend unavailable, rotation in flight, journal write failure, malformed receipt.

## P14 dep-chain

**Upstream**:
- `crates/anima/anima-identity/src/{custody,p256}.rs` — production `AnimaCustody` trait + 6 backends
- `crates/ergon/ergon-life-hooks/src/attestation.rs` — existing `SoulAttester` trait + `AnimaAttestHook`
- `crates/ergon/ergon/src/hook.rs` — `HookCtx` exposes `session_id`, `workflow_name`, `step_index`, `parent_session_id`
- `crates/lago/lago-auth/` — Rust-side auth library
- `apps/broomva/lib/lago-auth/verify-jwt.ts` (broomva.tech, M9-G) — downstream verifier reference shape
- Spec D anima-custody at `docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md` (rotation semantics)
- Spec C₃ §11.2 dependency rules

**Downstream**:
- BRO-1226 implementation follow-up (the actual signing body)
- BRO-1001 — arcan adapter wires the production `AnimaAttestHook` against this adapter
- broomva.tech AAP verifier — extends to handle `typ=agent+receipt+jwt` (small change; file as `[BRO-1217-follow]` once the receipt shape is approved)
- `crates/lago/lago-auth/` — optional Rust-side receipt verifier for in-process re-ingest

## Open questions (deferred to implementation ticket)

1. **`HookCtx` access from `ResponseScorer::score`**. Same question as BRO-1225 §Open §1 — applies to attestation too. The current ergon hook signatures pass `HookCtx`; the new `AgentAttestationSigner::sign_step_receipt` will need similar plumbing (or take a struct with the relevant fields pre-extracted).

2. **`AttestationJournal` trait**. Where do signed JWS strings get written? Options: (a) inject an `Arc<dyn AttestationJournal>` into the adapter; (b) emit a `tracing::event!` with the JWS and let lago's bridge pick it up; (c) emit directly onto an injected `aios_kernel::EventSink`. Each has tradeoffs — defer to implementation review.

3. **Canonical-JSON serializer**. Rust ecosystem options: `serde_jcs`, `canonical_json`, hand-rolled. The choice affects portability with the TS-side verifier (M9-G uses a known canonicalization). Defer to implementation — but the JCS RFC 8785 path is the safest cross-language default.

4. **Tool-call fingerprints**: §4 lists `tool_calls` as part of the receipt. Should fingerprints include tool *args + result* or just the *result*? Args are usually already in the audit trail; result is the surface that "what happened" depends on. Lean toward both, but profile receipt size before committing.

5. **Receipt batching**. One JWS per step × thousands of steps per session = many small JWS strings. Batch into one JWS per N steps with an array body? Defer — measure first.

6. **`typ=agent+receipt+jwt` agreement with the broomva.tech AAP verifier**. M9-G verifier currently expects `typ=agent+jwt`. The receipt verifier extension needs PR review on the broomva.tech side before this implementation can land — call out the PR-pair coordination explicitly in the impl ticket.

## Acceptance (per BRO-1226)

- [x] ADR at `docs/architecture/adr/2026-05-22-anima-signing-surface-for-ergon-attestation.md`
- [x] 6 design questions answered with chosen direction + justification each
- [x] Skeleton trait `AgentAttestationSigner` committed at `crates/ergon/ergon-anima-adapter/src/lib.rs`
- [x] M9-G verifier compatibility: noted — JWS shape matches existing verifier; `typ` extension is the only required change (no signature-format break). PR-pair coordination noted in §Open §6.
- [ ] **Review**: 1+ human reviewer on the Ergon project (open after PR opens)

## Backreferences

- BRO-994 — Ergon v0.1 umbrella (Done)
- BRO-1001 — arcan adapter ticket (frames v0.1 production wiring)
- BRO-1217 — M9-G AAP verifier (Done; downstream consumer for receipt JWS shape)
- BRO-1225 — sibling ADR (Nous adapter for Ergon scoring; same paper-only pattern)
- Decision 2 option (c) from 2026-05-21 orchestration session
- Spec D anima-custody — `docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md`
- `crates/anima/anima-identity/src/custody.rs:202` — `AnimaCustody` trait (the existing substrate)
- `crates/ergon/ergon-life-hooks/src/attestation.rs:30` — `SoulAttester` trait (existing session-boundary contract)
- `apps/broomva/lib/lago-auth/verify-jwt.ts` — M9-G AAP verifier (downstream consumer)
