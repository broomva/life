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

# METALAYER

This repository operates as a control loop for autonomous agent development.

## Setpoints (calibrated 2026-02-28)

- pass_at_1 target: 1.00 (alert below 0.90)
- retry_rate target: 0.10 (alert above 0.30)
- merge_cycle_time target: 24h (alert above 48h)
- revert_rate target: 0.03 (alert above 0.08)
- human_intervention_rate target: 0.15 (alert above 0.35)

## Sensors

- CI checks
- Test outcomes
- Web E2E outcomes
- CLI E2E outcomes
- Static checks
- Runtime traces/logs

## Controller Policy

- Gate sequence: smoke -> check -> test
- Retry budget: 2 (per gate, per run)
- Escalation conditions: retry_budget_exhausted -> human_oncall

## Actuators

- Code edits
- Script updates
- Policy updates
- Documentation updates
- Hook and workflow updates

## Feedback Loop

1. Measure
2. Compare
3. Decide
4. Act
5. Verify
