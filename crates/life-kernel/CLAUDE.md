# life-kernel

Implementation of the aiOS kernel contract for the µVM isolation tier.

Spec: `../../../../docs/superpowers/specs/2026-04-23-lifed-kernel-daemon-design.md`

## Crates (Phase 1 will flesh these out)

- `life-kernel-proto` — ttrpc wire contract (NOT YET CREATED; Phase 1)
- `life-kernel-core` — `KernelPort` impl + gate chain + metering (NOT YET CREATED; Phase 1)
- `life-kernel-gate` — gate port impls (NOT YET CREATED; Phase 1)
- `life-kernel-conformance` — per-backend test harness (SCAFFOLD ONLY in Phase 0)
- `lifed` (binary) — the daemon (NOT YET CREATED; Phase 2)

## Phase 0 status

Only `life-kernel-conformance` exists as an empty scaffold, so the workspace
resolves and downstream tooling works. Phase 1 fills in the actual suite.

## Dependency rules

- life-kernel-* MAY depend on: aios-protocol, arcan-sandbox, arcan-provider-*, lago-core, life-vigil
- life-kernel-* MUST NOT depend on: arcand, arcan-core, arcan-harness
