---
tags:
  - broomva
  - life
  - control
  - governance
type: operations
status: active
area: governance
created: 2026-03-17
---

# Control Loop

The autonomous development loop for this repository. All control primitives are consolidated here.

## Loop Definition

1. **Measure**: Capture sensor outputs (CI results, test outcomes, static checks).
2. **Compare**: Compute error against setpoints (see Metrics below).
3. **Select**: Choose control action based on policy (.life/control/policy.yaml).
4. **Execute**: Run command/action through gate sequence (smoke → check → test).
5. **Verify**: Re-measure and persist results to .life/control/state.json.

## Gate Sequence

```
smoke (cargo check)  →  check (fmt + clippy)  →  test (cargo test)
                                                       ↓
                                              recover (if failures)
                                                       ↓
                                              escalate (if retries exhausted)
```

- Retry budget: 2 per gate per run
- Escalation: `retry_budget_exhausted → human_oncall`

## Sensors

| Sensor | Required Fields | Sampling | Source |
|---|---|---|---|
| CI checks | check_name, status, duration_ms | every push/PR | GitHub Actions |
| Harness events | trace_id, run_id, status, duration_ms | always | scripts/control/*.sh |
| Test outcomes | test_name, pass/fail, duration_ms | every run | cargo test |
| Architecture audit | invariant_id, pass/fail | every audit | verify_dependencies.sh |
| Conformance suite | test_id, pass/fail | every audit | conformance/run.sh |

## Setpoints

| Metric | Target | Alert Threshold |
|---|---|---|
| pass_at_1 | 1.00 | < 0.90 |
| retry_rate | 0.10 | > 0.30 |
| merge_cycle_time | 24h | > 48h |
| revert_rate | 0.03 | > 0.08 |
| human_intervention_rate | 0.15 | > 0.35 |

## Actuators

| Action | Preconditions | Postconditions | Rollback |
|---|---|---|---|
| Patch code | Tests defined for change | All checks green | Revert commit |
| Update harness docs | Doc owner identified | Docs aligned with code | Restore prior version |
| Tune CI workflow | CI dry run passes | Stable runtime | Revert workflow file |
| Adjust policy | Escalation triggered | New gate enforced | Restore prior policy |
| Run recovery | Failure detected | Gates re-evaluated | Escalate to human |

## Stability

### Disturbance Scenarios

| Scenario | Expected Behavior | Recovery Target |
|---|---|---|
| Dependency upgrade | Temporary check failures | Recover within 1 day |
| Major feature branch | Higher variance in metrics | Recover within sprint |
| Infrastructure outage | Degraded CI signal | Recover when infra restored |
| New crate addition | Architecture audit may flag | Fix boundaries same session |

### Stabilization Playbook

1. Reconfirm setpoints against actual performance.
2. Reduce surface area of active change.
3. Enforce stricter checks temporarily.
4. Run entropy cleanup (remove stale docs/scripts/rules).

## Escalation

Escalate when:
- Retries exceed budget (2 per gate)
- Hard policy rules are violated (no-merge-with-failing-checks)
- Architecture invariants broken
- Human-required decision detected

## Observability Events

Required event types for control instrumentation:
- `control.step.start` — gate begins
- `control.step.success` — gate passes
- `control.step.failure` — gate fails
- `control.escalation` — human intervention triggered

Required fields per event: `run_id`, `trace_id`, `task_id`, `command_id`, `status`, `duration_ms`

## Control Frequency

| Loop | Cadence | Trigger |
|---|---|---|
| Fast (per change) | Every commit/PR | Git hooks, CI workflows |
| Agent tick | Every agent tick | Arcan GET /gating/{session_id} → Autonomic |
| Daily | 04:00 UTC | control-nightly.yml |
| Weekly | Manual | Review setpoint drift |

## Autonomic Feedback Loop (Agent Runtime)

The development control loop (above) governs the build/test pipeline. The Autonomic feedback loop governs agent runtime behavior through a second, nested control system.

### Nested Control Architecture

```
Outer loop (development):  Commit → Smoke → Check → Test → Audit → Deploy
Inner loop (runtime):      Event → Fold → Rules → Gating → Agent Tick → Event
```

### Inner Loop Definition

1. **Observe**: Fold events from Lago journal into `HomeostaticState` (deterministic, pure).
2. **Evaluate**: Run 6 rules against state, each producing `Option<GatingDecision>`.
3. **Merge**: Most-restrictive-wins across all firing rules.
4. **Actuate**: Return `AutonomicGatingProfile` via HTTP GET (advisory, fail-open).
5. **Execute**: Arcan applies gating profile to tool permissions, model selection, rate limits.
6. **Emit**: Agent tick produces events → back to step 1 (closed loop).

### Loop Properties

| Property | Status | Notes |
|---|---|---|
| Stability | PASS | Bang-bang with hysteresis — no oscillation in deadband |
| Observability | PARTIAL | Observer correct, sensor disconnected (separate journals) |
| Controllability | PASS | Every pillar has rules mapping deviations to corrections |
| Monotonic safety | PASS | Most-restrictive merge prevents any rule weakening another |
| Fail-safe | PASS | Advisory boundary — Autonomic down → base policy continues |
| Determinism | PASS | Same events → same state (projection is pure fold) |
| Closed-loop | **OPEN** | Events not yet flowing from plant to observer (R5 Phase 2) |
