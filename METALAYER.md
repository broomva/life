# METALAYER

This repository operates as a **control loop for autonomous agent development**, governed by the [agentic-control-kernel](../agentic-control-kernel/) metalayer.

**Core Law**: Do not grant an agent more mutation freedom than your evaluator can reliably judge.

## Control Flow

```
Plant (Life Agent OS)
  → observe() — CI gates, runtime health, economic state, journal metrics
  → Estimator (Autonomic) — three-pillar homeostatic projection
  → Belief state b_t — AgentStateVector + HomeostaticState
  → LLM Agent (Arcan) — request decision(b_t) → θ_t (typed control directive)
  → Controller (arcan-core) — propose(b_t, θ_t) → proposed u_t
  → Safety Shield (composite: policy-gate + capability-gate + budget-gate + sandbox)
    → filter(u_t, b_t) → safe u_t + certificate
  → Plant — apply(safe u_t) → result
  → Evaluator/Ledger (Lago) — append trace + score
```

## Plant Interface

- **Plant ID**: `life-agent-os`
- **Plant type**: Cyber (discrete state, human-scale cadence)
- **Config**: `.life/control/plant.yaml`
- **State schema**: `schemas/state.schema.json`
- **Action schema**: `schemas/action.schema.json`

## Setpoints (calibrated 2026-03-19)

| Metric | Target | Alert |
|--------|--------|-------|
| pass_at_1 | 1.00 | < 0.90 |
| gate_pass_rate | 1.00 | < 0.90 |
| audit_pass_rate | 1.00 | < 0.95 |
| constraint_violation_rate | 0.00 | > 0.05 |
| shield_intervention_rate | 0.00 | > 0.10 |
| shield_feasibility_rate | 1.00 | < 0.95 |
| retry_rate | 0.10 | > 0.30 |
| merge_cycle_time | 24h | > 48h |
| revert_rate | 0.03 | > 0.08 |
| human_intervention_rate | 0.15 | > 0.35 |

## Multi-Rate Hierarchy

| Loop | Cadence | LLM? | Controller | What runs |
|------|---------|-------|------------|-----------|
| Inner (servo) | N/A | No | — | Not applicable (cyber plant) |
| Mid (constrained) | N/A | No | — | Not applicable (no MPC/CBF-QP) |
| Outer (supervisory) | ~5s | Yes | arcan-core | Agent tick: reconstruct → provider → execute → stream |
| Meta (EGRI) | min–days | Yes | autoany-aios | Controller synthesis, policy evolution, model learning |

## Sensors

| Sensor | Source | Cadence |
|--------|--------|---------|
| Smoke gate | scripts/control/smoke.sh | pre-commit |
| Check gate | scripts/control/check.sh | pre-push, PR |
| Test gate | scripts/control/test.sh | PR, CI |
| Control audit | scripts/audit_control.sh | PR, nightly |
| Architecture audit | scripts/architecture/verify_dependencies.sh | PR, nightly |
| Heartbeat | scripts/control/heartbeat.sh | hourly |
| Runtime health | /health endpoints (Arcan, Lago, Autonomic, Haima) | continuous |
| Economic state | Autonomic /api/state | per-tick |
| Journal metrics | Lago event count, blob store size | per-session |

## Safety Shield (Composite)

| Layer | Type | Description |
|-------|------|-------------|
| Policy Gate | Rule-based | Hard rules from `.life/control/policy.yaml` — block on failing checks, require plans |
| Capability Gate | Rule-based | aiOS CapabilityPolicy — per-tool approval based on OperatingMode |
| Budget Gate | Threshold | Autonomic economic gating — block when budget exhausted |
| Sandbox Boundary | Containment | Praxis FsPolicy — workspace boundary enforcement |

**Fallback**: On shield infeasibility → OperatingMode::Recover → escalate to AskHuman.
**Saturation alert**: shield_intervention_rate > 0.10 triggers investigation.

## Controller Policy

- **Profile**: Autonomous (upgraded 2026-02-28)
- **Gate sequence**: smoke → check → test → audit
- **Retry budget**: 2 (per gate, per run)
- **Escalation**: retry_budget_exhausted → human_oncall
- **Failure model**: Environment-first triage before code-level fault attribution

## EGRI Integration

- **Problem spec**: `assets/templates/problem-spec.control.yaml`
- **Execution backend**: Local (cargo test across workspaces)
- **Ledger**: Lago (`egri.` event prefix via EventKind::Custom)
- **Autonomy mode**: Sandbox (promote requires constraint check)
- **Mutation surface**: Agent loop, homeostasis rules, economic modes, gating profiles

## Typed Schemas

| Schema | Path | Purpose |
|--------|------|---------|
| State | `schemas/state.schema.json` | Plant/belief state (measured + estimated + context + constraints) |
| Action | `schemas/action.schema.json` | Control directive θ_t (setpoint_update, mode_switch, etc.) |
| Trace | `schemas/trace.schema.json` | Ledger entry (state + directive + shield + outcome + evaluator) |
| Evaluator | `schemas/evaluator.schema.json` | Score vectors + promotion decisions |
| EGRI Event | `schemas/egri-event.schema.json` | Trial records for Lago persistence |

## Actuators

- Code edits (agent loop, rules, policies)
- Script updates (gates, heartbeat, E2E)
- Policy updates (.life/control/*.yaml)
- Documentation updates (STATUS.md, ARCHITECTURE.md)
- Hook and workflow updates (.githooks/, .github/workflows/)

## Feedback Loop

```
1. Measure   — Run sensors (CI gates, audits, heartbeat, runtime health)
2. Compare   — Check measured vs setpoints (.life/control/policy.yaml)
3. Decide    — Controller selects action (θ_t) based on belief state
4. Shield    — Safety filter ensures action is safe (composite shield)
5. Act       — Apply safe action (code edit, policy update, mode switch)
6. Verify    — Re-run sensors, confirm improvement
7. Record    — Append trace to Lago journal (schemas/trace.schema.json)
```

## Consciousness Stack

| Layer | Location | Purpose |
|-------|----------|---------|
| Working memory | Context window | Current session reasoning |
| Auto-memory | `~/.claude/.../memory/` | Cross-session agent learning |
| Conversation logs | `docs/conversations/` | Permanent episodic memory |
| Knowledge graph | `docs/`, `~/broomva-vault/` | Architectural patterns, wikilinks |
| Policy rules | `.life/control/policy.yaml` | Enforceable constraints |
| Typed schemas | `schemas/` | Canonical interfaces |
| Invariants | `CLAUDE.md` | Foundational truths |

## Cross-Project Philosophy

- Harness/control are feedback systems for continuous feature development.
- Local hooks and PR CI enforce the same intent (early detection, deterministic checks, safe progression).
- Failures are control signals; unresolved capability gaps are environment debt.
- The LLM emits typed directives (θ_t), not raw actuations. Controllers execute, shields filter, plants obey.
- Every action produces an immutable trace in the Lago journal.
