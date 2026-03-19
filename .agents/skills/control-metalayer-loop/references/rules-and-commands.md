# Rules And Commands

Keep command and rule governance explicit and versioned.

## Rule Types

- Hard gates: non-negotiable checks.
- Soft policies: preferred behavior with override paths.
- Escalation rules: when autonomy must hand off to human.
- Recovery rules: rollback, retry, or de-scope actions.

## Command Governance

Expose a stable command surface through wrappers:

- `make smoke`
- `make check`
- `make test`
- `make web-e2e`
- `make cli-e2e`
- `make hooks-install`
- `make recover`
- `make control-audit`

Keep direct tooling (`cargo`, `npm`, `pytest`) behind wrapper scripts for portability and deterministic behavior.

## Command Contract Pattern

For each command:

- Preconditions
- Expected outputs
- Failure modes
- Recovery action
- Escalation path

Store this in `.control/commands.yaml`.

## End-To-End Validation

- Web changes require browser-level E2E checks against deployed or preview URLs.
- CLI changes require binary-level E2E checks using real command invocations.
- Keep these checks in dedicated workflows so failures are isolated and actionable.
