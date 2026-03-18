---
tags:
  - broomva
  - life
  - control
  - operations
type: operations
status: active
area: governance
created: 2026-03-17
---

# Control Heartbeat Strategy

This heartbeat strategy keeps autonomous development loops trustworthy across projects.

## Purpose

Heartbeat checks are not feature tests. They validate that the **feedback system itself** is healthy:
- environment capabilities exist
- hooks and CI guardrails are active
- harness/control audits still pass

## Philosophy

1. **Feedback-first**: every failure should improve the next iteration.
2. **Environment-first triage**: assume missing capability/access before blaming feature code.
3. **Same control law at all levels**:
   - local hooks (fast feedback)
   - PR CI (branch feedback)
   - heartbeat/nightly (drift feedback)

## Cadence

- Pre-commit: `check`
- Pre-push: `test`
- PR pipeline: `smoke -> check -> test -> audit`
- Hourly heartbeat: capability + invariant checks
- Nightly strict audit: drift and entropy detection

## Heartbeat Checks

1. Toolchain prerequisites (`git`, `make`, `cargo`, `jq`)
2. Capacity guard (minimum disk headroom before heavy build/test loops)
3. Control artifacts (`.control/policy.yaml`, `.control/commands.yaml`, `.control/topology.yaml`)
4. Hook wiring (`core.hooksPath = .githooks`)
5. Baseline control audit
6. Baseline harness audit

## Failure Handling

On heartbeat failure:
1. classify as environment/capability issue first
2. remediate access/tooling/service
3. re-run heartbeat and audits
4. only then escalate as code defect

## Output Contract

- Success: `HEARTBEAT_OK`
- Failure: `HEARTBEAT_ALERT: <concise issues>`
