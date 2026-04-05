# Feedback Loop

Define how observations produce corrective actions for the Spaces development loop.

## Loop Steps

1. **Measure**: Run `make smoke` (WASM compile + client compile), `make check` (clippy), harness/control audits. Collect pass/fail, durations, error messages.
2. **Compare**: Check results against setpoints — compile times within budget? All gates green? Audit scores at 100%? Reducer error rate below 2%?
3. **Decide**: If any gate fails, identify root cause category:
   - Schema issue → fix `tables.rs` or `types.rs`, regenerate bindings
   - Reducer logic → fix in `reducers/`, republish
   - Client issue → fix `src/main.rs`, rebuild
   - Infra issue → check maincloud status, retry
   - Docs gap → fill in missing sections
4. **Act**: Apply minimal fix. If schema changed: regenerate bindings. If reducer changed: republish module.
5. **Verify**: Re-run the failing gate. Confirm `spacetime logs spaces` shows no new errors. Run full `make ci` for confidence.

## Control Frequency

- Fast loop (per change): `make smoke` on every code edit — takes <90s, catches compile regressions immediately
- Per-commit loop: Full `make ci` (smoke + check + test) — runs on every push via GitHub Actions
- Daily loop: Review `spacetime logs spaces` for accumulated reducer errors or unexpected patterns
- Weekly loop: Review setpoint metrics (compile times, error rates, audit scores), update METALAYER.md if targets need adjustment

## Error Budget Policy

- Error budget metric: CI gate failures as percentage of total commits in rolling 7-day window
- Budget window: 7 days
- Budget exhaustion response: When failure rate exceeds 15%, freeze new feature work and focus exclusively on stabilization — fix flaky tests, clean up warnings, ensure all gates are reliably green before resuming feature development
