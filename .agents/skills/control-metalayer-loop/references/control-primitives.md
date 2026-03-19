# Control Primitives

Use this map to reason about autonomous repo development as a dynamic control system.

## Mapping

- Plant: repository + CI + runtime behavior.
- Controller: policy rules + decision logic + supervising humans.
- Actuators: agent edits, command execution, PR operations.
- Sensors: tests, static checks, logs, traces, eval outcomes.
- Setpoints: target quality, reliability, speed, autonomy.
- Disturbances: requirement changes, dependency updates, outages, flaky tests.

## Minimal Control Law

1. Run `smoke`.
2. If `smoke` fails, stop and fix environment/build issues only.
3. Run `check` (`lint + typecheck`).
4. If `check` fails, block merge and repair static issues.
5. Run `test`.
6. If `test` fails, allow bounded retries; then escalate.
7. If failures persist across runs, tighten policy or reduce change surface.

## Required Metrics

- pass_at_1
- retry_rate
- time_to_actionable_failure
- merge_cycle_time
- revert_rate
- human_intervention_rate

## Stability Criteria

- Bounded retries.
- Decreasing regression frequency.
- Consistent audit pass rate.
- Controlled entropy (docs/scripts/rules in sync).
