# life-runtime

The `life-runtime` crate cluster ships the **public-facing** surface of the Life Agent OS — the boundary at which apps, browsers, CLIs, and external agents call into the framework.

## Spec ground truth

- **Master spec:** `docs/superpowers/specs/2026-04-25-life-runtime-architecture-spec.md` §L0–§L14
- **Spec C₂ (lifed facade):** `docs/superpowers/specs/2026-04-26-spec-c2-lifed-facade.md`
- **M5 implementation plan:** `docs/superpowers/plans/2026-04-26-m5-lifed-build.md`

## Crates

| Crate | Role |
|---|---|
| `lifed` (binary + lib) | Facade-aggregator daemon. Hosts `life.v1.{Agent, Events, Wallet, Identity}` and `life.admin.v1.{Runtime, Saga, RoutingCache}` over UDS. Stateless except for a routing cache rebuildable from lago. Saga-orchestrates cross-substrate writes. |
| `arcan-proxy` | Typed tonic client for the arcan substrate. With-token builder + retry policy. (Sub-phase A: stub; sub-phase B fills in.) |
| `lago-proxy` | Same, for lago. |
| `haima-proxy` | Same, for haima. |
| `anima-proxy` | Same, for anima. |
| `life-runtime-proto` | Generated proto types for `life.v1.*` + `life.admin.v1.*`. Mirrors the `aios-proto` codegen pattern; uses `extern_path` to reuse the canonical `aios.v1.*` types instead of regenerating them. |
| `lifed-conformance` | Substrate-token verification battery per Spec C₂ §15.5. (Sub-phase A: scaffold; sub-phase B task B17 populates the battery.) |

## Phase status

- **M5 sub-phase A** — Agent + Events with mock substrates: SHIPPED 2026-04-26 (BRO-930).
- **M5 sub-phase B** — Wallet + Identity + real proxies + saga compensation: pending.
- **M5 sub-phase C** — Admin plane: pending.
- **M5 sub-phase D** — Connection pools + circuit breakers + backpressure + observability: pending.
- **M5 sub-phase E** — Integration + verification + bake-in: pending.

## Dependency rules

Per Spec C₂ §11.2:

- `lifed` MAY depend on: `aios-protocol`, `aios-proto`, `life-runtime-proto`, `life-kernel-proto` (client features only — for the SpawnChild saga's soma admin call), the four `*-proxy` crates, `life-vigil`, transport/utility crates from §10.2.
- `lifed` MUST NOT depend on: any substrate runtime crate (`arcand`, `arcan-core`, `arcan-harness`, `arcan-aios-adapters`, `lago-runtime` family, `haima-runtime` family, `anima-runtime` family, `life-kernel-core`, `life-kernel-gate`, `life-kernel-facade`, `arcan-provider-*`).
- The four `*-proxy` crates depend ONLY on: `aios-protocol`, `aios-proto`, the substrate's wire crate (e.g. `life-kernel-proto` for soma), `tonic`, light utility crates. They MUST NOT pull substrate runtime crates either.

Enforced by `core/life/scripts/verify_dependencies_lifed.sh` and the `verify-deps-lifed` CI lane.

## Sub-phase A surface

* `lifed daemon` boots, binds the configured public UDS, applies mode + group, accepts a tonic client connection.
* `tower::Layer`-mounted auth middleware runs Tier-2 validation before any handler.
  * Sub-phase A: dev signer accepts `Bearer test-token-for-{user_id}` and synthesises minimal `CapabilityClaims`.
  * Sub-phase B5: real ES256 + JWKS verification.
* `Agent` service: 11 RPCs against trait-abstracted dispatch surfaces (`ArcanDispatch`, `LagoDispatch`, `HaimaDispatch`, `AnimaDispatch`). `CreateSession` does serial dispatch through all four mock substrates and seeds the routing cache. `SpawnChild` returns `Status::unimplemented` per Spec C₂ §13 (post-MVS / Spec C₇).
* `Events` service: `Read`, `Subscribe`, `GetBlob` against a `LagoTail` trait. Sub-phase A returns canned empty streams + a single-byte blob.
* Routing cache: `DashMap<String, Arc<RwLock<RouteEntry>>>` with `by_user` index. No eviction yet (sub-phase B8).
* In-memory idempotency store with 24h TTL sweeper (sub-phase B7 swaps to lago).
* No-op saga driver + 4 `CreateSession` step stubs (sub-phase B6 + B11 fill in).
