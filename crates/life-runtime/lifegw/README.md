# lifegw — Life Runtime Edge Gateway (M7)

`lifegw` is the **stateless, internet-facing edge gateway** of the Life Runtime —
the only daemon with a public TCP socket. Spec C₃ at
[`docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md`](../../../docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md)
is the canonical implementation reference; this README is a navigation aid.

## Sub-phase status

- **Sub-phase A** (BRO-935, merged): scaffolding + TLS bind + dev-mode JWT
  acceptance + Tier-2 mint via static dev keystore + `tonic-web` unary proxy
  passthrough to lifed UDS + `/healthz`.
- **Sub-phase B** (BRO-936, merged): real Vercel JWKS + ES256/RS256 verifier
  with kid lookup + 30 min rotation grace + algorithm allowlist; KMS-backed
  Tier-2 mint via the new `KmsSigner` trait (StaticKeystore + VaultTransit
  primary, AwsKms / GcpKms feature-gated); JWKS published atomically to
  `/run/life/lifegw-jwks.json`; TLS 1.3-only listener.
- **Sub-phase C** (BRO-938, this PR): WebSocket upgrade handler at
  `/v1/agent/stream` + bidi pump (browser ↔ lifed `Agent.StreamSession`) +
  reconnect-by-`last_seq_no` via header / query param + close-code policy
  per Spec C₃ §6.5 + per-WS bounded mpsc(64) backpressure with slow-consumer
  detector. Three Sub-phase B follow-ups closed: `KmsProvider::Dev` is now
  fail-closed unless `dev_signer_enabled = true`; `Tier1Claims.tier`
  propagates into `Tier2Claims.tier`; route-scope intersection enforced
  before Tier-2 mint per Spec C₃ §5.4.
- **Sub-phase D** (planned): per-user / per-IP token-bucket rate limit +
  admin-plane UDS + cert-watch + JWKS single-flight refactor.
- **Sub-phase E** (planned): production KMS swap-in + chaos tests.

> ## ⚠️ PRODUCTION CUTOVER GATE — apps/chat JWKS bridge required
>
> Sub-phase B verifies Tier-1 JWTs against a JWKS endpoint at the configured
> `auth.jwks_url`. The default points at `https://broomva.tech/api/auth/jwks.json`
> — but **apps/chat does not yet publish that endpoint** (Spec C₃ §16 #1
> open question). Until apps/chat ships the Better-Auth → JWT bridge that
> publishes a Vercel-style JWKS at `/api/auth/jwks.json`, **DO NOT roll lifegw
> to production with `dev_signer_enabled = false`** — every verify will fail
> on first call (no JWKS endpoint to fetch). For staging / integration
> testing, the wiremock-based test harness in `tests/integration_jwks_round_trip.rs`
> is the canonical pattern.
>
> Production cutover blocker tracked as a follow-up ticket against Spec C₃
> §16 #1. The blocker resolves when apps/chat (Better-Auth) ships the bridge.

## What ships in Sub-phase C

| Subsystem | State |
|---|---|
| TLS bind via rustls — **TLS 1.3 only** | shipped (B) |
| Dev-mode JWT acceptance (`Bearer dev-token-for-{user_id}`) | preserved behind `JwksCache::dev_only` |
| Real Vercel JWKS Tier-1 verifier (ES256 + RS256, alg allowlist) | shipped (B) |
| Tier-2 mint via `KmsSigner` trait (Vault primary; Static dev) | shipped (B) |
| Atomic JWKS publish to `/run/life/lifegw-jwks.json` | shipped (B) |
| `tonic-web` Connect protocol layer | shipped (A) |
| `life.v1.{Agent, Events, Wallet, Identity}` unary proxy passthrough | shipped (A) |
| `/healthz` upstream-readiness check | shipped (A) |
| **WebSocket upgrade at `/v1/agent/stream` + bidi pump** | shipped (C) |
| **Reconnect-by-`last_seq_no` via header / query param** | shipped (C) |
| **Close-code policy per Spec C₃ §6.5 (1000, 1001, 1008, 1011, 4001-4005)** | shipped (C) |
| **Per-WS bounded mpsc(64) + slow-consumer detector** | shipped (C) |
| **Route-scope intersection enforcement before Tier-2 mint** | shipped (C) |
| **`KmsProvider::Dev` fail-closed unless `dev_signer_enabled`** | shipped (C) |
| **`Tier1Claims.tier` propagates to Tier-2** | shipped (C) |
| Rate limiting + admin-plane UDS | deferred to Sub-phase D |
| AWS / GCP KMS provider bodies | deferred to Sub-phase E |

## WebSocket protocol (Sub-phase C)

The WS surface is mounted on `/v1/agent/stream`. Auth + scope intersection
runs BEFORE the upgrade response is sent — a forbidden bearer never gets a
101 Switching Protocols response.

**Required parameters** (one of each):
- Session id: `?sid=<sid>` query param OR `X-Life-Sid: <sid>` header.
- Optional resume cursor: `?last_seq_no=<u64>` OR `X-Life-Last-Seq-No: <u64>`.
- Bearer Tier-1 JWT: `Authorization: Bearer <jwt>` (the gateway swaps this
  for a Tier-2 capability before forwarding to lifed).

**Frame format** (Spec C₃ §6.2): JSON envelope.

Server → client:
```json
{ "kind": "agent_event", "seq_no": 4232, "record": {...}, "agent_kind": "TOKEN" }
{ "kind": "pong", "seq_no": 0 }
{ "kind": "closing", "reason": "rate_limit:per_user" }   // pre-close diagnostic
```

Client → server:
```json
{ "kind": "send_message",     "content": "Hello" }
{ "kind": "approve_dispatch", "dispatch_id": "disp-42" }
{ "kind": "cancel_dispatch",  "dispatch_id": "disp-42" }
{ "kind": "ping",             "seq_no": 5 }
{ "kind": "close" }
```

Unknown frame kinds drop silently with a `frame_drop` debug log
(metric coming in Sub-phase D).

**Close codes** (Spec C₃ §6.5):

| Code | Meaning | Reason string |
|------|---------|---------------|
| 1000 | Normal closure | `normal` |
| 1001 | Server going away | `going_away` |
| 1008 | Policy violation (token expired) | `policy_violation:token_expired` |
| 1011 | Internal error / heartbeat timeout | `internal_error` |
| 4001 | Rate limit (Sub-phase D wires) | `rate_limit:per_user` |
| 4002 | Slow consumer / backpressure | `backpressure:slow_consumer` |
| 4003 | IP blocked | `ip_blocked` |
| 4004 | lifed unavailable | `lifed_circuit_open` |
| 4005 | Sequence retired (`out_of_range`) | `sequence_retired` |

Note: code 1008 is the standard WebSocket "policy violation" code we use
for token-expired (vs the prompt's suggested 4001) so we don't conflict
with §6.5's reserved 4001 = rate-limit slot.

**Backpressure** (Spec C₃ §8.2): each WS connection holds two 64-message
bounded mpsc channels (inbound / outbound). The slow-consumer detector
samples the outbound channel capacity once per second; after 5 consecutive
samples at capacity 0, the connection closes with `4002`.

## TLS feature audit (Sub-phase B decision)

Per Spec C₃ §6 + master spec §L4, lifegw negotiates **TLS 1.3 only**. The
rustls + tokio-rustls dependencies are pinned with `default-features = false,
features = ["ring", "std"]` — the `tls12` feature is intentionally OFF.

Rationale:
- Modern clients (browsers, Vercel edge, mobile SDKs) support TLS 1.3.
- TLS 1.2 carries CBC-mode + RSA key-exchange + downgrade (POODLE-class) risk.
- Removing TLS 1.2 simplifies the audit surface — fewer cipher suites,
  fewer code paths.
- Aligns with the "asymmetric signing only" spirit of the master spec
  invariant 1.

If a future tenant requires TLS 1.2 fallback, that is an explicit policy
decision tracked under a new ticket — not a default.

## Quick start

```bash
# Build the daemon.
cargo build -p lifegw --bin lifegw

# Run with a dev config (binds [::]:8443 with a self-signed cert).
LIFEGW_CONFIG=crates/life-runtime/lifegw/lifegw.example.toml \
  cargo run -p lifegw -- daemon
```

## Dependency rules

Per Spec C₃ §11, lifegw MUST NOT depend on:

- substrate runtime crates (`arcand`, `arcan-core`, `lago-runtime` family,
  `haima-runtime` family, `anima-runtime` family, `life-kernel-*`)
- substrate proxy crates (`arcan-proxy`, `lago-proxy`, `haima-proxy`,
  `anima-proxy`)
- the `lifed` runtime crate

`scripts/verify_dependencies_lifegw.sh` enforces these rules in CI.

## Tests

```bash
cargo test -p lifegw                   # unit tests + integration tests
bash scripts/verify_dependencies_lifegw.sh
```
