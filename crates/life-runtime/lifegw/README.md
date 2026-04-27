# lifegw — Life Runtime Edge Gateway (M7)

`lifegw` is the **stateless, internet-facing edge gateway** of the Life Runtime —
the only daemon with a public TCP socket. Spec C₃ at
[`docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md`](../../../docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md)
is the canonical implementation reference; this README is a navigation aid.

## Sub-phase status

- **Sub-phase A** (BRO-935, this PR): scaffolding + TLS bind + dev-mode JWT
  acceptance + Tier-2 mint via static dev keystore + `tonic-web` unary proxy
  passthrough to lifed UDS + `/healthz`.
- **Sub-phase B** (BRO-936, planned): real Vercel JWKS + ES256 + KMS-backed
  Tier-2 mint + scope intersection table.
- **Sub-phase C** (BRO-937, planned): WebSocket upgrade + bidi pump + reconnect.
- **Sub-phase D** (BRO-938, planned): per-user / per-IP token-bucket rate limit
  + bounded backpressure + admin-plane UDS + cert-watch.
- **Sub-phase E** (BRO-939, planned): production KMS swap-in + chaos tests.

## What ships in Sub-phase A

| Subsystem | State |
|---|---|
| TLS bind via rustls (TLS 1.2/1.3 default) | shipped |
| Dev-mode JWT acceptance (`Bearer dev-token-for-{user_id}`) | shipped |
| Tier-2 capability token mint via in-process P-256 keystore | shipped |
| `tonic-web` Connect protocol layer | shipped |
| `life.v1.{Agent, Events, Wallet, Identity}` unary proxy passthrough | shipped |
| `/healthz` upstream-readiness check | shipped |
| Real ES256 + Vercel JWKS | deferred to Sub-phase B |
| WS upgrade + reconnect | deferred to Sub-phase C |
| Rate limiting + bounded buffers | deferred to Sub-phase D |

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
