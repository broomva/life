# Control Loop

## Loop Definition

1. **Measure**: Run `make smoke` to verify WASM module and CLI client compile. Run `make check` for clippy. Run audits for artifact completeness. Check `spacetime logs spaces` for reducer errors.
2. **Compare**: Evaluate against setpoints:
   - Compile times within budget? (WASM <60s, client <30s)
   - All gates green? (smoke, check, test)
   - Audit scores at 100%? (21/21 harness, 15/15 control)
   - Reducer error rate <2%?
3. **Select control action**:
   - If WASM compile fails → schema or reducer fix in `spacetimedb/src/`
   - If client compile fails → fix `src/main.rs` or regenerate bindings
   - If clippy fails → apply suggested fixes
   - If publish fails → check maincloud, verify schema compatibility
   - If audit fails → add missing docs/scripts
   - If all green but slow → investigate build caching, dependency reduction
4. **Execute**: Apply the minimal fix. Follow the actuator catalog (see ACTUATORS.md). If schema changed, run `spacetime generate` then rebuild client. If reducer changed, run `spacetime publish`.
5. **Verify**: Re-run the failing gate. If green, run full `make ci` for confidence. Check `spacetime logs spaces` for clean output. Persist results in CI artifacts.

## Escalation

Escalate when:
- Retry count exceeds 2 for the same gate step on the same change
- A schema migration requires `--clear-database` (destructive, needs explicit approval)
- Maincloud is unreachable for >10 minutes (infra issue, outside agent control)
- A SpacetimeDB SDK breaking change affects >3 files (major adaptation required)
- Hard policy violation: attempting to bypass auth checks, committing credentials, force-pushing to main
