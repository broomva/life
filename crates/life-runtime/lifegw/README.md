# lifegw — Life Runtime Edge Gateway (M7)

`lifegw` is the **stateless, internet-facing edge gateway** of the Life Runtime —
the only daemon with a public TCP socket. Spec C₃ at
[`docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md`](../../../docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md)
is the canonical implementation reference; this README is a navigation aid.

## Sub-phase status

- **Sub-phase A** (BRO-935, merged): scaffolding + TLS bind + dev-mode JWT
  acceptance + Tier-2 mint via static dev keystore + `tonic-web` unary proxy
  passthrough to lifed UDS + `/healthz`.
- **Sub-phase B** (BRO-936, this PR): real Vercel JWKS + ES256/RS256 verifier
  with kid lookup + 30 min rotation grace + algorithm allowlist; KMS-backed
  Tier-2 mint via the new `KmsSigner` trait (StaticKeystore + VaultTransit
  primary, AwsKms / GcpKms feature-gated); JWKS published atomically to
  `/run/life/lifegw-jwks.json`; TLS 1.3-only listener.
- **Sub-phase C** (BRO-937, planned): WebSocket upgrade + bidi pump +
  reconnect.
- **Sub-phase D** (BRO-938, planned): per-user / per-IP token-bucket rate
  limit + bounded backpressure + admin-plane UDS + cert-watch.
- **Sub-phase E** (BRO-939, planned): production KMS swap-in + chaos tests.

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

## What ships in Sub-phase B

| Subsystem | State |
|---|---|
| TLS bind via rustls — **TLS 1.3 only** (Sub-phase B decision; see below) | shipped |
| Dev-mode JWT acceptance (`Bearer dev-token-for-{user_id}`) | preserved behind `JwksCache::dev_only` |
| Real Vercel JWKS Tier-1 verifier (ES256 + RS256, alg allowlist) | shipped |
| Tier-2 mint via `KmsSigner` trait (Vault primary; Static dev) | shipped |
| Atomic JWKS publish to `/run/life/lifegw-jwks.json` | shipped |
| `tonic-web` Connect protocol layer | shipped |
| `life.v1.{Agent, Events, Wallet, Identity}` unary proxy passthrough | shipped |
| `/healthz` upstream-readiness check | shipped |
| WS upgrade + reconnect | deferred to Sub-phase C |
| Rate limiting + bounded buffers | deferred to Sub-phase D |
| AWS / GCP KMS provider bodies | deferred to Sub-phase E |

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
