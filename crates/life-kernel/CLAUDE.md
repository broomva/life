# life-kernel

Implementation of the aiOS kernel contract for the µVM isolation tier
(Spec A) plus the unified gateway for every other Life framework
capability (Spec B.1). The directory hosts:

- Spec A: `../../../../docs/superpowers/specs/2026-04-23-lifed-kernel-daemon-design.md`
- Spec B.1: `../../../../docs/superpowers/specs/2026-04-24-life-kernel-facade-design.md`

## Crates

- `life-kernel-proto` — tonic wire contract. Hosts the Spec A `KernelService` (post-M3: lives at `core/life/proto/life/kernel/v1/kernel.proto` in the canonical tree, imports `aios.v1.*` types from `aios-proto`) plus the Spec B.1 v0 service family — `common`, `events`, `session`, `approvals`, `policy` (still at `crates/life-kernel/life-kernel-proto/proto/`; migrated in M3.5/M4) — and v0.2 reserved stubs for `tools`, `model`, `relay`. The Phase 1 spec called for ttrpc-rust; `ttrpc-codegen 0.6` is incompatible with the `prost 0.14` ecosystem the workspace has standardised on, so the crate emits tonic client + server stubs from the same `.proto`. See the `build.rs` header for the full rationale.
- `aios-proto` — generated proto types from `core/life/proto/aios/v1/*.proto` (canonical Layer-2 wire vocabulary). Crate at `crates/aios/aios-proto/`. Internal substrates use `aios-protocol` (Layer-1 hand-written types) and bridge to `aios-proto` (Layer-2 generated) at the wire boundary via `aios_protocol::proto_bridge` (M3 / BRO-928).
- `life-kernel-core` — `KernelPort` engine composing `BackendRegistry` + `GateChain` + `MeteringWrapper` over any `HypervisorBackend`. Pure state machine; reconstructable from the Lago event stream via `KernelEngine::replay`. (SHIPPED in Phase 1, #974.)
- `life-kernel-gate` — Phase 1 MVS gate impls: `NoOpBudgetGate`, `NoOpNetworkIsolation`, `StaticPolicyGate` (wraps `aios-policy::PolicyGatePort`). Real budget + network impls land in Phase 4. (SHIPPED in Phase 1, #974.)
- `life-kernel-conformance` — backend-agnostic conformance battery (lifecycle / errors / metering / events). 100% green against `arcan-provider-local` through `life-kernel-core`. (SCAFFOLDED in Phase 0, FLESHED OUT in Phase 1.)
- `life-kernel-facade` — Spec B.1 v0 proxies (`EventsProxy` over lagod HTTP/SSE, `SessionProxy` + `ApprovalsProxy` over arcand) plus generic tonic service adapters that project the `aios-protocol` port traits onto the wire surface. v0.2 stubs (`ToolsService`, `ModelService`, `RelayService`) mounted but every method returns `Status::unimplemented`. SHIPPED Spec B.1 Phase 1.
- `life-client` — typed Rust client over the v0 tier (`Kernel`, `Events`, `Session`, `Approvals`, `Policy` handles). Unix socket primary; vsock + TCP feature-gated. SHIPPED Spec B.1 Phase 1.
- `soma` (binary) — daemon + operator CLI in one binary. `soma daemon` hosts the `KernelEngine` (Unix + vsock transport, Lago + Vigil wired, replay-on-restart, graceful shutdown, systemd unit). `soma create-vm` / `soma dispatch` / `soma list-vms` are operator subcommands over the tonic Unix-socket contract. (Renamed from `lifed` + `lifectl` in 2026-04-25 per Spec C M0; the original Phase 2 ship was #1014.) Future ticket will register the v0 services from `life-kernel-facade` on the same `/run/life/soma.sock`.

## Phase status

- **Spec A Phase 0** — ABI Foundation: shipped (#963). `aios-protocol` additive extensions, `HypervisorBackend` promotion, conformance scaffold.
- **Spec A Phase 1** — Kernel Proto + Core Library: shipped (#974). Four library-tier crates live; engine proven as a deterministic fold over the event journal.
- **Spec A Phase 2** — soma Daemon + Observability: **SHIPPED** (#1014, originally as `lifed` + `lifectl`; renamed `soma` 2026-04-25 per Spec C M0). Boxes the Phase 1 library inside a systemd-managed binary; Lago + Vigil wiring; operator subcommands; end-to-end tests. Plan: `docs/superpowers/plans/2026-04-24-lifed-phase-2-daemon.md`.
- **Spec B.1 Phase 0** — Facade ABI Foundation: shipped (#1002). 10 new port traits + DTO modules, 7 schema-only crates, meta-crate `schema` features.
- **Spec B.1 Phase 1** — v0 Core Services: shipped (#1003). `life-kernel-proto` extended with `common` + 4 v0 service protos + 3 v0.2 stubs; `life-kernel-facade` and `life-client` live; integration harness round-trips through a temp Unix socket without a binary. Plan: `docs/superpowers/plans/2026-04-24-life-kernel-facade-phase-1-v0-core.md`.
- **Spec B.1 Phase 2–4** — v0.1 + CLI migration + v0.2 lit-up. Queued.
- **Spec A Phases 3–5** — `arcan-provider-cube`, real gates, arcand cutover. Parallelisable now that Phase 2 has shipped.
- **Spec C M3** — Proto Consolidation: shipped (BRO-928). Canonical proto tree at `core/life/proto/`; `kernel.proto` migrated to `proto/life/kernel/v1/` and now imports the `aios.v1.*` vocabulary from `proto/aios/v1/identifiers.proto`. New `aios-proto` crate hosts the generated Layer-2 types; `aios_protocol::proto_bridge` ships round-trip impls for the 5 canonical identifier types (M3.5/M4 extends the bridge to compound types and migrates the 8 v0 service protos).

## Dependency rules

- `life-kernel-proto` MAY depend on: `aios-protocol`, `aios-proto` (plus `prost` / `tonic` generated scaffolding).
- `life-kernel-core` MAY depend on: `aios-protocol`, `arcan-sandbox`, `arcan-provider-*`, `life-kernel-proto`, `life-kernel-gate`, `lago-core`, `life-vigil`.
- `life-kernel-gate` MAY depend on: `aios-protocol`, `aios-policy`, `autonomic-core` (behind feature).
- `soma` binary MAY depend on every crate above.
- `life-kernel-*` MUST NOT depend on: `arcand`, `arcan-core`, `arcan-harness`, `arcan-aios-adapters`.

Enforced by `scripts/verify_dependencies_soma.sh`.
