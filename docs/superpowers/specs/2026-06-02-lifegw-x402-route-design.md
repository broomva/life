# Design — lifegw x402-initiate route (BRO-1346 / BRO-1341 step 1)

- **Status**: Accepted (decisions locked 2026-06-02) — P1 slice 1 in progress (BRO-1346)
- **Locked decisions**: (1) settlement **sync** for P1; (2) approval = server-side `PaymentPolicy` cap for P1; (3) funding via base-sepolia faucet (manual P1 step); (4) `X402Pay` extends the existing **Wallet substrate service**. Slice order: **1) haima x402-pay core (this slice)** → 2) gRPC/proxy/lifed/lifegw transport → 3) mainnet behind the financial gate.
- **Linear**: BRO-1346 (this design) · parent epic BRO-1341 · related BRO-1320 (Anima Base-capability spike), broomva.tech BRO-1340 (account onchain-identity reconcile)
- **Scope**: paper design + route contract; implementation lands after the open decisions (below) are locked.

## Context

broomva.tech now presents the **Anima wallet** as the user's native, Base-capable onchain identity (BRO-1340). The missing capability: let a user/agent **initiate an x402 payment** from that wallet to an external x402-enabled service. The x402 *client* machinery already exists in `haima-x402`; what's missing is the **edge route + RPC plumbing** to drive it, signed by the user's Anima-custodied key.

This is a **wiring** design, not a from-scratch build.

## Building blocks that already exist

| Block | Where | Note |
|---|---|---|
| x402 client flow | `crates/haima/haima-x402` — `X402Client::handle_402()`, `parse_payment_required`, `encode_payment_signature`, `parse_payment_response`, `Eip3009Authorization` | full GET→402→sign→retry→settle client |
| Wallet signing | `crates/haima/haima-wallet` — `WalletBackend::sign_transfer_authorization()` (EIP-3009), `hash_transfer_authorization()`, `USDC_BASE_MAINNET/SEPOLIA` domains | secp256k1 EIP-3009 USDC signing |
| **Anima↔wallet binding** | `crates/haima/haima-x402/src/custody_adapter.rs` — `CustodyWalletAdapter` (feature `custody-adapter`) | `impl WalletBackend` that routes `sign_transfer_authorization` → `AnimaCustody::sign_eip712`. **The key piece — exists, feature-gated, unwired.** |
| AnimaCustody | `crates/anima/anima-identity/src/custody.rs` — `sign_eip712`, `wallet_address`, 6 backends (Vault/TPM/HW/soma/WebCrypto/InProcess) | per-user secp256k1 wallet half |
| lifed→haima | `crates/life-runtime/haima-proxy` — `HaimaCall` trait, `HaimaProxy`, `Pooled`, Tier-3 token attach | typed tonic client + pool/breaker |
| lifegw route pattern | `lifegw/src/bootstrap.rs:545` (router compose), `scope.rs:127` (`enforce`), `services/anima_custody.rs` (state+handler), `proxy.rs` (`*Forwarder`) | `/anima/custody/*` is the copy-from template |

## Gaps this design fills

1. No lifegw route to initiate x402.
2. No `HaimaCall::x402_pay` proxy method.
3. No haimad gRPC method exposing `X402Client`.
4. No `RequiredScope::X402Pay` variant in `scope.rs`.
5. `CustodyWalletAdapter` not wired into the daemon's per-user wallet construction.

## Design

### Route contract (edge)

```
POST /haima/x402/pay              (Tier-1 JWT, scope x402:pay)
  body: {
    resourceUrl: string,          // the x402-protected resource to fetch+pay
    network: "base-sepolia" | "base",   // default base-sepolia (testnet-first, see Decision 3)
    maxAmount?: string,           // human-decimal cap; reject if 402 asks more
    method?: "GET"|"POST", body?  // the underlying request to replay after payment
  }
  → 200 { status:"settled", resource:<bytes/json>, settlement:{ tx, amount, asset, network }, receiptEventId }
  → 402 { status:"payment_declined", reason }    // policy/cap/insufficient-funds
  → 400 { error }  401 unauthorized  502 upstream
```

### Path (mirrors `/anima/custody/*`)

```
broomva.tech edge proxy  /api/x402/pay
   → lifegw  POST /haima/x402/pay        [Tier-1 verify + scope::enforce(x402:pay)]
      → lifed  (new X402Forwarder)        [Tier-2/Tier-3 token]
         → haima-proxy  HaimaCall::x402_pay(user_id, project_id, req)
            → haimad gRPC  Haima/X402Pay
               → X402Client { wallet: CustodyWalletAdapter(resolve AnimaCustody for user) }
                  GET resourceUrl → 402 → PaymentPolicy gate → sign (AnimaCustody.sign_eip712)
                  → retry X-PAYMENT → facilitator (CDP) verify+settle → resource + receipt
                  → emit Lago finance events (finance.TaskBilled / finance.RevenueReceived)
```

### Custody binding (the important part)

The payment is signed **by the user's Anima wallet**, not a haima-owned key:

- haimad, handling `X402Pay` for user `U`, resolves `U`'s `AnimaCustody` backend (per Spec D — Vault/TPM/etc.) and wraps it in `CustodyWalletAdapter` as the `WalletBackend`.
- `sign_transfer_authorization` → `hash_transfer_authorization` (EIP-712 digest) → `AnimaCustody::sign_eip712` → recoverable secp256k1 signature from the user's key.
- ⇒ the EIP-3009 `transferWithAuthorization` is authorized by the same address shown as the Anima wallet on `/account`. One identity, end to end.

### Gates (BRO-1341 hard constraints)

1. **`PaymentPolicy`** (haima-core: auto-approve under limit / require-approval / deny) gates every payment; `maxAmount` is a per-call cap on top.
2. **Network default = `base-sepolia`** (testnet). `base` (mainnet) requires the route's `network:"base"` AND a server-side `X402_MAINNET_ENABLED` flag AND the user's explicit financial sign-off (P2 control gate). No mainnet payment ships in phase 1.
3. **Scope `x402:pay`** — a `WalletWrite`-class scope; not granted to read-only tokens.
4. Every payment + settlement is a Lago event (auditable, replayable).

### Phasing

- **P1 (this work)**: route + proxy + haimad `X402Pay` RPC + wire `CustodyWalletAdapter`; **base-sepolia only**; `PaymentPolicy` enforced; Lago events; tests + a testnet round-trip against a sample x402 endpoint.
- **P2**: broomva.tech edge proxy `/api/x402/pay` + minimal UI (trigger from the Anima wallet on `/account`), feature-flagged.
- **P3**: mainnet enablement behind the financial control gate + approval UX.

## Open decisions (need lock before implementation)

1. **Settlement sync vs async** — block the HTTP response until the facilitator settles (simple, slower), or return `requestId` + settle async with a status poll (mirrors the Base MCP approval-mode shape)? *Recommend: sync for P1 (testnet, simple), async in P2.*
2. **Where the policy approval lives** — auto-approve under a per-project cap in haima `PaymentPolicy` (server) vs. an interactive approval (like the anima approval URL). *Recommend: server cap for P1; interactive approval is P3 mainnet.*
3. **Wallet funding** — the Anima wallet needs testnet USDC/ETH to pay. Faucet flow for base-sepolia; documented manual step for P1.
4. **`X402Pay` as a new gRPC method on the existing Wallet substrate service vs. a new Haima x402 service** — *Recommend: extend the Wallet service (reuses the existing proxy/pool/scope wiring).*

## Validation (P11)

- Rust: `cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace` in `core/life`.
- Unit: x402 handler with a mock facilitator + a stub `AnimaCustody` (assert the signed EIP-3009 authorization binds the user's address + the policy gate rejects over-cap).
- Integration: testnet round-trip against a sample base-sepolia x402 endpoint; assert settlement tx + Lago receipt event.
- No mainnet path exercised in P1.

## As-built — slice 2 transport (BRO-1354, 2026-06-03)

Built across one PR with staged compile-green layer commits. Decision #4 was honoured on **both planes**: `X402Pay` extends the existing Wallet service at the substrate plane (`haima.v1.WalletSubstrate`) **and** the public plane (`life.v1.Wallet`) — the only shape that respects lifegw's "no haima-proxy dependency" rule while reusing the existing proxy/pool/scope wiring.

| Layer | Where | What |
|---|---|---|
| proto (substrate) | `proto/haima/v1/substrate.proto` | `X402Pay(X402PayReq)→X402PayResp`; flat resp with a `status` discriminant (`settled`/`not_required`/`declined`) mirroring `X402PayResult`. |
| haimad | `crates/haima/haimad/src/substrate.rs` | `SubstrateService::x402_pay` — resolve custody → `CustodyWalletAdapter::from_custody_on_network(_, base_sepolia)` → `X402Client` → `pay_x402` → map. |
| proxy | `crates/life-runtime/haima-proxy/src/client.rs` | `HaimaCall::x402_pay` + `X402PayOutcome` (HaimaProxy + Pooled + MockHaima). |
| proto (public) | `proto/life/v1/wallet.proto` | `life.v1.Wallet.X402Pay` mirror. |
| lifed | `crates/life-runtime/lifed/src/services/wallet.rs` | `WalletService::x402_pay` → `haima.x402_pay` (not idempotency-cached — a settled on-chain payment can't be replayed; EIP-3009 nonce makes a resubmit a fresh auth). |
| lifegw (gRPC) | `proxy.rs` + `auth/scope.rs` | `WalletForwarder::x402_pay` (verbatim forward) + `RequiredScope::X402Pay` (`x402:pay`, disjoint from `wallet:write`). |
| lifegw (HTTP) | `services/haima_x402.rs` | bespoke `POST /haima/x402/pay` JSON route: verify Tier-1 → enforce `x402:pay` → mint Tier-2 → dial lifed. The edge surface slice P2 targets. |

**Open decisions — resolved:** (1) sync settlement ✓; (2) server-side `PaymentPolicy` cap ✓; (3) faucet/funding documented (manual P1 step, deferred to the live round-trip); (4) extend the Wallet service ✓.

**Custody (P1, documented deviation).** haimad resolves a per-`(user_id, project_id)` **deterministic `InProcessAnima`** (SHA-256-seeded via `derive_custody_seed`), not yet the soma-resident `/account` Anima wallet. This exercises the full sign + policy + settlement path on base-sepolia. **Slice 2b** swaps `resolve_custody` to `SomaCustody`/`RemoteAnima` behind the same handler — a localized change, no wire impact. Until then the x402 signing address is stable per `(user, project)` but distinct from both the substrate `GetBalance` wallet and the production Anima wallet.

**Open caveat — Gate #4 (Lago audit event) NOT yet met (P20 cross-review MEDIUM).** A settled x402 payment currently emits **no** `finance.*` Lago event — the substrate's `FinancePublisher` is a daemon-wide F2 stub (`Debit`/`Transfer` don't emit either; see `substrate.rs` header). So BRO-1341 Gate #4 ("Every payment + settlement is a Lago event — auditable, replayable") is **outstanding**, tracked with the F2 ledger work. Acceptable for the testnet transport slice (no real funds) but **must close before mainnet (slice 3)**.

**Deferred to follow-ups:**
- The live base-sepolia round-trip against a real funded wallet + facilitator config (P1 manual step).
- **Slice 2b** — swap `resolve_custody` from `InProcessAnima` to the soma-resident `SomaCustody`/`RemoteAnima` so the signing key IS the `/account` Anima wallet.
- **Lago finance event** on settle (Gate #4, with the F2 publisher).
- **Cross-user payer binding on the other Wallet RPCs** — `x402_pay` now binds the payer to the capability subject (P20 fix); `get_balance`/`debit`/`transfer` still trust the body `user_id` (pre-existing). Backfill the same guard as a separate hardening.
- **Network-consistency assertion** — the response `network`/`recipient` reflect the server-advertised 402 scheme; the signature is correctly domain-bound to the resolved network regardless, but `handle_402` should reject a 402 whose advertised network ≠ the resolved signing network (belongs with the mainnet-enablement work in the engine, slice 2b/3).
- broomva.tech edge proxy + UI (slice P2); mainnet (slice 3, financial control gate); `method`/`body` request replay in the JSON route body.

**Validation:** `cargo fmt --all --check` ✓, `cargo clippy --workspace --all-targets -- -D warnings` ✓, workspace tests ✓ (haimad 14, lifegw scope 19 + haima_x402 8, lifed wallet 7 incl. x402); both `verify_dependencies_{lifed,lifegw}.sh` ✓. (Unrelated pre-existing load-sensitive flake in `chronosd`'s heartbeat test — passes idle on clean main + this branch.)
