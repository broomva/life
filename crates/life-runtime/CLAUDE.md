# life-runtime

The `life-runtime` crate cluster ships the **public-facing** surface of the Life Agent OS — the boundary at which apps, browsers, CLIs, and external agents call into the framework.

## Spec ground truth

- **Master spec:** `docs/superpowers/specs/2026-04-25-life-runtime-architecture-spec.md` §L0–§L14
- **Spec C₂ (lifed facade):** `docs/superpowers/specs/2026-04-26-spec-c2-lifed-facade.md`
- **Spec C₃ (lifegw edge gateway):** `docs/superpowers/specs/2026-04-27-spec-c3-lifegw-design.md`
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

Enforced by `core/life/scripts/verify_dependencies_lifed.sh` and the `Verify lifed dependency rules` CI lane.

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
