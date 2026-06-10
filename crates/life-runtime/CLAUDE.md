# life-runtime

The `life-runtime` crate cluster ships the **public-facing** surface of the Life Agent OS — the boundary at which apps, browsers, CLIs, and external agents call into the framework.

## Spec ground truth

- **Master spec:** `docs/superpowers/specs/2026-04-25-life-runtime-architecture-spec.md` §L0–§L14
- **Spec C₂ (lifed facade):** `docs/superpowers/specs/2026-04-26-spec-c2-lifed-facade.md`
- **Spec C₃ (lifegw edge gateway):** `docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md`
- **Spec D (anima production custody):** `docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md` — user-scoped sibling of Spec C₃ §5; defines `AnimaCustody` trait + 6 backends (P-256 auth, secp256k1 wallet, split-custody for browser).
- **M5 implementation plan:** `docs/superpowers/plans/2026-04-26-m5-lifed-build.md`

## Crates

| Crate | Role |
|---|---|
| `lifed` (binary + lib) | Facade-aggregator daemon. Hosts `life.v1.{Agent, Events, Wallet, Identity}` and `life.admin.v1.{Runtime, Saga, RoutingCache}` over UDS. Stateless except for a routing cache rebuildable from lago. Saga-orchestrates cross-substrate writes. |
| `arcan-proxy` | Typed tonic client for the arcan substrate. `ArcanCall` trait + `ArcanProxy` builder + retry policy + Tier-3 token attachment hook + Sub-phase E `Pooled<C>` adapter. |
| `lago-proxy` | Same shape, for lago. `LagoCall` trait + `LagoProxy` + retry policy + Sub-phase E `Pooled<C>` adapter. |
| `haima-proxy` | Same shape, for haima. `HaimaCall` trait + `HaimaProxy` + retry policy + Sub-phase E `Pooled<C>` adapter. Wallet types live here. |
| `anima-proxy` | Same shape, for anima. `AnimaCall` trait + `AnimaProxy` + retry policy + Sub-phase E `Pooled<C>` adapter. Account/Profile types live here. |
| `life-runtime-pool` | Sub-phase E shared crate. `Pool` (semaphore + circuit breaker + ArcSwap-able tonic Channel), `PoolGuard`, `CircuitBreaker` (with HalfOpen single-trial CAS), `BreakerState`, `SubstrateKind`, `SubstratePools`. Both `lifed` and the four `*-proxy` crates depend on it. |
| `life-runtime-proto` | Generated proto types for `life.v1.*` + `life.admin.v1.*`. Mirrors the `aios-proto` codegen pattern; uses `extern_path` to reuse the canonical `aios.v1.*` types instead of regenerating them. |
| `lifed-conformance` | Substrate-token verification battery per Spec C₂ §15.5. `SubstrateUnderTest` trait + `run_battery` + `reference_verify`. Body populated in Sub-phase B; exercised by `conformance_substrate_tokens.rs`. |

## Phase status

### M7 lifegw (edge gateway, BRO-935..939)

- **M7 sub-phase A** ✅ SHIPPED — TLS bind + dev-mode JWT + tonic-web unary proxy + `/healthz`.
- **M7 sub-phase B** ✅ SHIPPED 2026-04-28 — Real Vercel JWKS verifier + KMS provider abstraction + atomic JWKS publish + TLS 1.3-only listener (BRO-936, PR #1057 → main `37b89a3`).
- **M7 sub-phase C** ✅ SHIPPED — WebSocket upgrade at `/v1/agent/stream` + bidi pump (browser ↔ lifed `Agent.StreamSession`) + reconnect-by-`last_seq_no` (header / query) + close-code policy per Spec C₃ §6.5 + per-WS bounded mpsc(64) backpressure + 3 B-phase follow-ups closed (`KmsProvider::Dev` fail-closed, `Tier1Claims.tier` propagation, route-scope intersection enforcement). Refactor: bootstrap now drives connections via `hyper_util::server::conn::auto::Builder::serve_connection_with_upgrades` since tonic 0.14's `Server::serve_with_incoming_shutdown` doesn't enable hyper upgrades (BRO-938).
- **M7 sub-phase D** ✅ SHIPPED 2026-04-29 — Rate limit + admin plane + cert-watch + heartbeat + JWKS single-flight + 9 bundled M7-B/C follow-ups closed (D1–D11). Linear MCP re-auth pending; commits reference BRO-932 / BRO-938.
- **M7 sub-phase E** — Production KMS swap-in + chaos tests (planned).

### M7 Sub-phase D — handoff state (2026-04-29)

Sub-phase D operationalised lifegw: rate limit + admin plane + cert reload + heartbeat enforcement, plus 9 bundled follow-ups from M7-B/C reviews. The gateway is now production-deployable from a control-plane perspective (Sub-phase E swaps the production KMS in and adds chaos tests).

- **D1 — Token-bucket rate limiter** (`services/rate_limit.rs`): per-user + per-IP fixed-point token-bucket with LRU eviction (10k cap). Per-user defaults from config (60 req capacity, 60 req/s refill); per-IP defaults 60 cap / 60 req/min. Mounted in the auth Layer post-Tier-1-verify, pre-Tier-2-mint so rejected traffic doesn't pay JWS-mint cost. Returns `Status::resource_exhausted("rate_limit:{per_user|per_ip}")` (NOT `unavailable` — distinct gRPC semantics; `resource_exhausted` maps to WS close 4001 via the existing close-code mapper). 10 unit tests + 1 integration test.
- **D2 — Admin plane UDS** (`admin/`, `proto/life/admin/gw/v1/gateway.proto`): new `life.admin.gw.v1.GatewayAdmin` service mounted on `/run/life/lifegw-admin.sock` (group `life-admin`, mode `0660`). Authn via SO_PEERCRED + group membership (mirroring lifed's `auth/peercred.rs` + `services/admin/policy.rs` pattern). 5 RPCs: `HealthCheck`, `CertReload`, `JwksDump`, `Blocklist_{Add,Remove,List}`, `RateLimit_Override`. Closed-by-default policy table. 6 admin integration tests + 5 unit tests for policy + 5 for blocklist + 3 for peercred.
- **D3 — Cert-watch + SIGHUP reload** (`services/cert_watch.rs`, `shutdown.rs`): `CertReloader` holds `ArcSwap<Arc<rustls::ServerConfig>>`. Polling-based file-watcher (5 s cadence) sidesteps the `notify` macOS/Linux divergence the prompt flagged. SIGHUP handler triggers immediate reload regardless of mtime (cert-rotation scripts that replace files atomically with same mtime are supported). Parse failures rejected — previous config stays live. Admin-plane `CertReload` RPC routes through the same reloader. 5 unit tests. **Caveat**: in-flight per-connection TLS acceptor swap (so already-accepted connections keep their config but new accepts use the new config) is wired through the admin RPC + SIGHUP handle but NOT yet plumbed into the public-plane `serve_connections` accept loop — that's a Sub-phase E refinement. The reload counter advances and the JWKS-side observability is already correct.
- **D4 — JWKS single-flight** (`auth/jwks.rs`): `parking_lot::Mutex<FlightCoalescer>` + `Condvar` so 100 concurrent kid-miss callers produce at most ONE upstream HTTP fetch. Cohort-based fail-closed propagation: a winner's fetch error is replicated to every waiter on the condvar. Bounds upstream Vercel JWKS rate-limit exposure during hot kid rotations. 3 unit tests.
- **D5 — WS heartbeat enforcement** (`services/ws.rs`): `HEARTBEAT_INTERVAL` (30 s) ping arm + `PONG_CHECK_INTERVAL` (1 s) pong-deadline check arm in the bidi-pump `tokio::select!`. `last_pong_at` updated on every WS pong frame. `PONG_DEADLINE` (60 s = 2 × heartbeat) tolerates a single dropped ping; two in a row trip a `1011 InternalError` close. Browser-initiated pings replied to inline. 2 unit tests + the constant-mapping sanity test.
- **D6 — `from_sequence` proto extension** (`proto/life/v1/agent.proto`, `services/ws.rs`, `lifed/services/agent.rs`): `optional uint64 from_sequence = 2;` on `SessionRef`. lifegw forwards the WS resume cursor as `from_sequence` on the upstream `Agent.StreamSession` request. lifed reads the field for operator-visible logging; live tail replay against the lago substrate is a Sub-phase E plumbing pass (the wire shape is now in place). Backwards-compatible (optional field).
- **D7 — `Arc<JwksCache>` per-`AuthService`** (`auth/middleware.rs`, `auth/dev_signer.rs`, `bootstrap.rs`): explicit handle threaded via `AuthLayer::with_jwks(...)`. The legacy `OnceLock<JwksCache>` global stays behind `#[deprecated]` shims (`install_tier1_verifier`, `verify`) until Sub-phase E removes them. Production hot path no longer touches global state. Unblocks per-test verifier swaps.
- **D8 — `build_signer` error-class consistency**: AWS/GCP feature-enabled arms now return `LifegwError::Config` (config-time failures at startup) instead of `Auth`.
- **D9 — Per-WS persistent dispatcher** (`services/ws.rs`): inbound `SendMessage` frames go through a bounded mpsc(64) consumed by a single dispatcher task that serialises upstream `Agent.SendMessage` calls. A misbehaving client sending 1000 frames in 100 ms now produces ≤1 active upstream stream + 64 queued commands instead of 1000 concurrent streams. The dispatcher exits cleanly when the WS closes.
- **Test counts** (all green on this branch): 109 lifegw unit tests + 4 conformance + 6 admin-plane integration + 1 jwks round-trip + 3 proxy-passthrough + 1 rate-limit integration + 5 ws-bidi integration = 129 total. Workspace baseline + new = ~3700+ → ~3729+.

Sub-phase D acceptance: `cargo build --workspace` clean, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --all -- --check` clean, `bash scripts/verify_dependencies_lifegw.sh` exits 0. All 9 bundled M7-B/C follow-ups closed (D6/D7/D8/D9 from M7-C reviews + the 4 hardening items folded into D1/D2/D3/D5).

### M5 lifed (facade, BRO-930..934)

- **M5 sub-phase A** ✅ SHIPPED 2026-04-26 — Agent + Events with mock substrates (BRO-930, PR #1047 → main `7f91f40`).
- **M5 sub-phase B** ✅ SHIPPED 2026-04-26 — Real substrate proxies (shape) + ES256/JWKS + real `SagaDriver` with reverse compensation + lago-backed idempotency + routing-cache eviction + full Wallet + Identity service bodies + multi-tab fanout registry + conformance battery body (BRO-931, PR #1050).
- **M5 sub-phase C** ✅ SHIPPED 2026-04-26 — Admin plane (`life.admin.v1.{Runtime, Saga, RoutingCache}`).
- **M5 sub-phase D** ✅ SHIPPED 2026-04-28 — Connection pools + circuit breakers + backpressure + observability + per-substrate `with_token` wiring + `JwksKey` PEM publish (BRO-934, PR #1058).
- **M5 sub-phase E** — Pool push-down + half-open trial CAS + RAII PumpGuard + OTLP exporter + 15 metric series wiring (BRO-937).

## Sub-phase E handoff state — pool push-down + observability hardening

Sub-phase E completed the M5 production-readiness pass:

- **Pool push-down (E1)**: each `*-proxy` crate now exposes a `Pooled<C>` adapter that wraps any inner `*Call` impl (real proxy or mock) and brackets every method through the shared `life_runtime_pool::Pool` (semaphore + circuit breaker + ArcSwap-able Channel). Wallet/Identity/Events services dropped their `pools` field — pool bracketing is uniform. The chaos test (`integration_circuit_breaker`) now drives the lago breaker through real `Agent.CreateSession` traffic; the `TestEnv::record_lago_failures` direct-bump stopgap is gone.
- **Half-open trial CAS (E2)**: `CircuitBreaker::half_open_trial_active: AtomicBool` gates entry into HalfOpen via `compare_exchange`. 100 concurrent `Pool::acquire` calls into HalfOpen now produce exactly one admitted guard; the other 99 short-circuit with `unavailable`. Tested under stampede.
- **RAII PumpGuard (E2)**: `lifed::services::agent::PumpGuard` wraps the per-session pump-active flag in a Drop-impl release. Panicking pump tasks release the slot — verified by `integration_pump_guard::pump_guard_releases_slot_on_panic`.
- **OTLP exporter (E2)**: `cfg.vigil.otlp_endpoint` now drives `life_vigil::init_telemetry`. The W3C `TraceContextPropagator` is installed globally so outbound substrate calls propagate `traceparent`. Logging-only fallback preserved when no endpoint is configured.
- **15 metric series (E4)**: every `life.{daemon,session,saga}.*` series from Spec C₂ §9.3 is incrementing in production code. Pool-level counters (`dispatch.count`, `dispatch.duration_ms`, `semaphore.inflight`, `breaker_state`) emit from inside `life-runtime-pool::PoolGuard`. Routing-cache + saga-driver + fanout-broadcast feed the rest. Handler latency captured by a tower middleware on both planes.
- **Lago wire RPC (E3) — DEFERRED**: lagod (`crates/lago/proto/lago/v1/`) hasn't shipped `lago.Append` or `lago.ListNamespaces` yet; the only typed RPCs are `IngestService.Ingest` + `CreateSession` + `GetSession`. The lago-proxy shim falls back to `idem_persist` (content-addressed dedup key) for `append_event` and returns an empty vec for `list_namespaces`. RoutingCache cold-start handles the empty-vec path by warming on incoming traffic. Tracked as a follow-up under BRO-934 (lago wire RPC roll-out).
- **`#[non_exhaustive]`** added to `RetryClass` (4 proxy crates), `BreakerState`, `SubstrateKind`. `SubstratePoolsInitial` + `SubstrateKind` now carry `///` doc comments on every variant.

Sub-phase E acceptance: 3640 baseline tests + 22 new = 3662 passing. `cargo build --workspace` clean, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --all -- --check` clean, `bash scripts/verify_dependencies_lifed.sh` exits 0.

## Sub-phase B handoff state — read this before starting C/D

Sub-phase B established **structural correctness**: every Sub-phase A mock has been replaced with real machinery, every Spec C₂ §3-§7 trait abstraction is in place, every error path is typed, every saga compensates in reverse on failure. What Sub-phase B did **not** do is wire all the per-RPC bodies end-to-end against running substrate daemons; the four `*-proxy` crates expose the right traits + builder shapes + error mappings, but the per-method bodies in `arcan-proxy/src/client.rs` (and siblings) remain canned stubs. Real RPC bodies and the connection-pool wrapping land in Sub-phase D. Code-quality review of PR #1050 surfaced these gaps explicitly:

- `*Proxy::with_token(...)` stores the token but no proxy method attaches it to outgoing tonic metadata yet → wire in D1.
- `auth/keystore.rs::JwksKey` writes a JWKS file with no `x`/`y`/`pem` material → consumers can't load it → fix shape in D2.
- `Wallet.Transfer` is not idempotent (`Debit` is) → align in D3 alongside haima per-task billing.
- `SagaCtx.deadline` carried but unenforced → wrap each step in `tokio::time::timeout_at` in D4.
- `spawn_fanout_pump` re-dials per `SendMessage` so two tabs cause two upstream pumps → refactor to one upstream pump per session in D5.
- `*ProxyError` retry-class info is erased at the `LifedError::Substrate` boundary → add `RetryClass` discrimination for D's pool.

Sub-phase C will independently surface:

- Lago saga-state persistence (Spec C₂ §4.1) so admin-plane `Saga.Show` has something to read.
- Per-sid `Agent.ApproveDispatch` lock (Spec C₂ §6.4 — currently the handler is a `Empty` ack stub).
- `bootstrap::run_with_real_substrates` mock-fallback gating behind an explicit `--allow-mock-fallback` flag so production deploys fail-fast on missing UDS.

These are tracked as D-wave follow-ups (BRO-XXX series filed against the Spec C umbrella).

## Dependency rules

Per Spec C₂ §11.2:

- `lifed` MAY depend on: `aios-protocol`, `aios-proto`, `life-runtime-proto`, `life-kernel-proto` (client features only — for the SpawnChild saga's soma admin call), the four `*-proxy` crates, `life-vigil`, transport/utility crates from §10.2.
- `lifed` MUST NOT depend on: any substrate runtime crate (`arcand`, `arcan-core`, `arcan-harness`, `arcan-aios-adapters`, `lago-runtime` family, `haima-runtime` family, `anima-runtime` family, `life-kernel-core`, `life-kernel-gate`, `life-kernel-facade`, `arcan-provider-*`).
- The four `*-proxy` crates depend ONLY on: `aios-protocol`, `aios-proto`, the substrate's wire crate (e.g. `life-kernel-proto` for soma), `tonic`, light utility crates. They MUST NOT pull substrate runtime crates either.

Per Spec C₃ §11.2 (lifegw extension, ratified 2026-05-18 [BRO-1164]):

- `lifegw` MAY depend on: `aios-protocol`, `aios-proto`, `life-runtime-proto`, `life-kernel-proto` (wire types ONLY — for the Spec D D-Sub-C anima custody routes that proxy to soma's `life.admin.kernel.v1.CustodyOracle` service via `services/anima_custody.rs`), `life-vigil`, transport/TLS/WS/JWT crates, standard utility crates.
- `lifegw` MUST NOT depend on: substrate runtime crates (`arcand`, `arcan-core`, `arcan-harness`, `arcan-aios-adapters`, `arcan-provider-*`, `arcan-sandbox`, lago/haima/anima runtime families), `life-kernel-{core,gate,facade}` (runtime/facade internals — proto is allowed per the carve-out above, the rest is not), the four `*-proxy` crates (lifed's south side), `lifed` itself (the gateway dials via `life-runtime-proto`'s tonic client).

The `life-kernel-proto` carve-out for lifegw is symmetric with lifed's allowance: both daemons consume the typed wire types to talk to soma's admin custody-oracle service, and neither links the runtime/facade crates. The carve-out is a precondition for Spec D D-Sub-C — the WebCryptoAnima / RemoteAnima browser path requires the gateway to issue `CustodyOracle` calls.

Enforced by `core/life/scripts/verify_dependencies_lifed.sh` and `verify_dependencies_lifegw.sh`, and the `Verify lifed dependency rules` + `Verify lifegw dependency rules` CI lanes.

### SIGPIPE bug history [BRO-1164]

From the original lifegw script's first deploy (2026-04-19) until 2026-05-18, the verify-deps scripts under `scripts/verify_dependencies_*.sh` carried a silent-FAIL bug: the check pattern `echo "$tree" | grep -qE "..."` ran under `set -o pipefail`. When `grep -q` matched early and closed the read end of the pipe, `echo` received SIGPIPE on the next write and the pipeline exited non-zero (signal 13). The `if echo ... | grep ...; then ...` shape interpreted the non-zero pipeline exit as **no match**, silently masking real violations on the CI Linux runners. The lifegw lane reported "all lifegw dependency rules pass" even though `life-kernel-proto` was already in the transitive tree from D-Sub-C anima custody routes. Local macOS runs hit the FAIL because timing/buffering differs, but CI ran green for ~1 month. Fix replaced `echo | grep` with `<<<` here-strings (no pipe → no SIGPIPE), and every script now ships a `--self-test` mode that injects a synthetic forbidden dep and asserts the FAIL path fires.

## Sub-phase A surface (preserved as historical record)

* `lifed daemon` boots, binds the configured public UDS, applies mode + group, accepts a tonic client connection.
* `tower::Layer`-mounted auth middleware runs Tier-2 validation before any handler.
  * Sub-phase A: dev signer accepts `Bearer test-token-for-{user_id}` and synthesises minimal `CapabilityClaims`.
  * Sub-phase B5: real ES256 + JWKS verification — invalid tokens return `Status::unauthenticated` early. Dev signer survives behind `JwksCache::dev_only()` for tests.
* `Agent` service: 11 RPCs against trait-abstracted dispatch surfaces. Sub-phase A used `*Dispatch` traits (e.g. `ArcanDispatch`); Sub-phase B collapsed them into the proxy crates' `*Call` traits (e.g. `ArcanCall`) so mocks `impl ArcanCall` directly without bridge adapters. `CreateSession` runs the real 4-step saga via `SagaDriver` with reverse compensation. `SpawnChild` returns `Status::unimplemented` per Spec C₂ §13 (post-MVS / Spec C₇).
* `Events` service: `Read`, `Subscribe`, `GetBlob` against a `LagoTail` trait. Sub-phase A returns canned empty streams + a single-byte blob; Sub-phase B keeps the same shape (real bodies land in D when lago-proxy methods stop stubbing).
* Routing cache: `DashMap<String, Arc<RwLock<RouteEntry>>>` with `by_user` index. Sub-phase B added idle-TTL + LRU-hard-cap eviction sweeper.
* Idempotency store: `IdempotencyStore` trait with both `InMemoryStore` (dev/tests) and `LagoBackedStore` (production); 24h TTL sweeper.
* Saga driver: real forward-then-reverse-compensate per Spec C₂ §4. Compensation failures are logged not retried per §4.2. State persistence to lago lands in C alongside admin `Saga.Show`.
