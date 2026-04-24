# life-kernel

Implementation of the aiOS kernel contract for the µVM isolation tier.

Spec: `../../../../docs/superpowers/specs/2026-04-23-lifed-kernel-daemon-design.md`

## Crates

- `life-kernel-proto` — tonic wire contract for `KernelService` (SHIPPED in Phase 1, #974). The Phase 1 spec called for ttrpc-rust; `ttrpc-codegen 0.6` is incompatible with the `prost 0.14` ecosystem the workspace has standardised on, so the crate emits tonic client + server stubs from the same `.proto`. See the `build.rs` header for the full rationale.
- `life-kernel-core` — `KernelPort` engine composing `BackendRegistry` + `GateChain` + `MeteringWrapper` over any `HypervisorBackend`. Pure state machine; reconstructable from the Lago event stream via `KernelEngine::replay`. (SHIPPED in Phase 1, #974.)
- `life-kernel-gate` — Phase 1 MVS gate impls: `NoOpBudgetGate`, `NoOpNetworkIsolation`, `StaticPolicyGate` (wraps `aios-policy::PolicyGatePort`). Real budget + network impls land in Phase 4. (SHIPPED in Phase 1, #974.)
- `life-kernel-conformance` — backend-agnostic conformance battery (lifecycle / errors / metering / events). 100% green against `arcan-provider-local` through `life-kernel-core`. (SCAFFOLDED in Phase 0, FLESHED OUT in Phase 1.)
- `lifed` (binary) — daemon hosting the `KernelEngine`; Unix + vsock transport, Lago + Vigil wired, replay-on-restart + graceful shutdown + systemd unit. (SHIPPED in Phase 2.)
- `lifectl` (binary) — operator CLI: `create-vm`, `dispatch`, `list-vms` over the tonic Unix-socket contract. (SHIPPED in Phase 2.)

## Phase status

- **Phase 0** — ABI Foundation: shipped (#963). `aios-protocol` additive extensions, `HypervisorBackend` promotion, conformance scaffold.
- **Phase 1** — Kernel Proto + Core Library: shipped (#974). Four library-tier crates live; engine proven as a deterministic fold over the event journal.
- **Phase 2** — lifed Daemon + Observability: **SHIPPED** (#1014). Boxes the Phase 1 library inside a systemd-managed binary; Lago + Vigil wiring; `lifectl` CLI; end-to-end tests. Plan: `docs/superpowers/plans/2026-04-24-lifed-phase-2-daemon.md`.
- **Phases 3–5** — `arcan-provider-cube`, real gates, arcand cutover. Parallelisable after Phase 2.

## Dependency rules

- `life-kernel-proto` MAY depend on: `aios-protocol` (plus `prost` / `tonic` generated scaffolding).
- `life-kernel-core` MAY depend on: `aios-protocol`, `arcan-sandbox`, `arcan-provider-*`, `life-kernel-proto`, `life-kernel-gate`, `lago-core`, `life-vigil`.
- `life-kernel-gate` MAY depend on: `aios-protocol`, `aios-policy`, `autonomic-core` (behind feature).
- `lifed` binary MAY depend on every crate above.
- `life-kernel-*` MUST NOT depend on: `arcand`, `arcan-core`, `arcan-harness`, `arcan-aios-adapters`.

Enforced by `scripts/verify_dependencies_lifed.sh`.
