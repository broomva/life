---
tags:
  - broomva
  - life
  - architecture
  - control
type: architecture
status: active
area: system
created: 2026-03-17
---

# Control-Aware Architecture

**Last updated**: 2026-03-03

System design for the control plane governing the `/life` monorepo.

---

## Components

```
.life/control/                          Declarative policy, commands, topology
  ├── policy.yaml                  Gate sequence, retry budget, escalation rules
  ├── commands.yaml                Canonical command definitions (smoke, check, test, recover)
  ├── topology.yaml                Zone ownership, criticality, agent permissions
  └── state.json                   Last audit timestamp, controller mode

Makefile.control                   Entrypoint for all control targets
  └── scripts/control/
        ├── smoke.sh               cargo check (aiOS, arcan, lago, spaces)
        ├── check.sh               cargo fmt --check + cargo clippy (×4)
        ├── test.sh                cargo test --workspace (×4)
        ├── recover.sh             Diagnostic recovery (identify failure, attempt fix, escalate)
        ├── install_hooks.sh       Wire .githooks/ into git config
        ├── cli_e2e.sh             Build + exercise CLI binaries
        └── web_e2e.sh             Start arcand + exercise HTTP API

scripts/audit_control.sh           Baseline + strict artifact existence audit
scripts/architecture/
  └── verify_dependencies.sh       Cross-project dependency boundary enforcement

conformance/run.sh                 8-suite canonical behavior validation

.githooks/
  ├── pre-commit                   → scripts/control/check.sh
  └── pre-push                     → scripts/control/check.sh (full)

.github/workflows/
  ├── control-harness.yml          CI: smoke → check → test → audit
  ├── control-nightly.yml          Scheduled entropy/drift detection
  ├── cli-e2e.yml                  CLI binary validation
  └── web-e2e.yml                  HTTP API validation
```

---

## Boundaries

### Interface Boundary
- Parse and validate external input at system edges (HTTP, CLI, gRPC).
- Control scripts accept env-var overrides (`CONTROL_SMOKE_CMD`, `APP_CLI_BIN`, `APP_BASE_URL`) for flexibility.

### Domain Boundary
- Operate on internal typed models within each Rust workspace.
- aiOS defines the contract; Arcan and Lago implement against it.

### Persistence Boundary
- Serialize state transitions through Lago's append-only journal.
- Control state persisted in `.life/control/state.json` (audit timestamps, controller mode).

### Control Boundary
- Control scripts never modify production code — they only measure, validate, and report.
- Recovery actions are bounded: format fixes only, then escalate.

---

## Ownership

| Zone | Owner | Criticality | Gate Requirements |
|------|-------|-------------|-------------------|
| `aiOS/` | Kernel contract | HIGH | fmt + clippy + test |
| `arcan/` | Runtime | HIGH | fmt + clippy + test + conformance |
| `lago/` | Persistence | HIGH | fmt + clippy + test + conformance |
| `autonomic/` | Homeostasis | HIGH | fmt + clippy + test |
| `praxis/` | Tool engine | HIGH | fmt + clippy + test |
| `spaces/` | Networking | HIGH | fmt + clippy + check |
| `docs/` | Shared | MEDIUM | Existence audit |
| `.life/control/` | Control plane | HIGH | Strict audit |
| `.github/workflows/` | CI | MEDIUM | Syntax validation |

---

## Gate Flow

```
              ┌─────────┐
              │  smoke   │  cargo check (×4 workspaces)
              └────┬─────┘
                   │ pass
              ┌────▼─────┐
              │  check   │  cargo fmt --check + cargo clippy (×4)
              └────┬─────┘
                   │ pass
              ┌────▼─────┐
              │   test   │  cargo test --workspace (×4)
              └────┬─────┘
                   │ pass
         ┌─────────▼──────────┐
         │       audit        │
         │  ┌───────────────┐ │
         │  │ control-audit │ │  File existence + content checks
         │  └───────┬───────┘ │
         │  ┌───────▼───────┐ │
         │  │ arch-audit    │ │  Dependency boundary validation
         │  └───────┬───────┘ │
         │  ┌───────▼───────┐ │
         │  │ conformance   │ │  8 canonical behavior suites
         │  └───────────────┘ │
         └────────────────────┘
```

Each gate failure blocks the next. Retry budget: 2 per gate per run. Exhausted retries trigger escalation to human.

---

## Dependency Audit Rules

Enforced by `scripts/architecture/verify_dependencies.sh`:

1. **aiOS** must not depend on Arcan, Lago, Autonomic, or Praxis implementation crates.
2. **Lago core** may only depend on `aios-protocol` (not Arcan crates).
3. **Praxis** depends only on `aios-protocol` — no Arcan, Lago, or Autonomic dependencies.
4. **Autonomic** depends on `aios-protocol` and `lago-core`/`lago-journal` — no Arcan dependencies.
5. **Arcan** may depend on aiOS, Lago, Praxis, and Autonomic through adapter boundaries only.
6. **No circular dependencies** between project workspaces.

Violations cause `make audit` to fail, blocking CI.

---

## Autonomic Control Dynamics

The Autonomic subsystem implements a discrete-time sampled-data feedback controller for agent behavior regulation. It follows classical control theory architecture with event-sourced state.

### Signal Flow

```
Events (Lago journal)
    │
    ▼
┌──────────────┐     x̂(k)      ┌────────────┐     u(k)     ┌──────────┐
│   Observer    │──────────────>│ Controller  │────────────>│  Plant    │
│  fold() over  │               │ 6 rules +   │  gating     │  Arcan   │
│  event stream │               │ merge()     │  profile    │  agent   │
└──────────────┘               └────────────┘             │  loop    │
                                     ^                     └──────────┘
                                     │                          │
                                     │ setpoints r(t)           │ events
                                     │ (rule thresholds)        │ e(k)
                                     │                          │
                                     └──────────── Lago ◄───────┘
```

### State Vector (Three Pillars)

```
x̂(t) = [x_op(t), x_cog(t), x_econ(t)]

x_op   = { error_streak, total_errors, total_successes, mode }
x_cog  = { total_tokens_used, tokens_remaining, context_pressure }
x_econ = { balance, lifetime_costs, burn_estimate, mode, hysteresis_gate }
```

### Controller Rules

| Rule | Pillar | Process Variable | Threshold | Corrective Action |
|------|--------|-----------------|-----------|-------------------|
| SurvivalRule | Economic | balance/burn ratio | < 2.0, < 1.0, = 0 | Mode escalation (Sovereign→Conserving→Hustle→Hibernate) |
| SpendVelocityRule | Economic | cost last 5 min | > 500k mc | Model→Budget, tokens→2048 |
| BudgetExhaustionRule | Economic | remaining token fraction | < 20% | Model→Budget, tokens→1024, restrict expensive tools |
| ContextPressureRule | Cognitive | context pressure | > 80% | Model→Standard, tokens→2048 |
| TokenExhaustionRule | Cognitive | remaining token fraction | < 10% | Max tool calls→2, tokens→1024 |
| ErrorStreakRule | Operational | error rate (min 5 events) | > 30% | Restrict side effects, max tool calls→3 |

### Merge Strategy

Most-restrictive-wins across all dimensions. Post-merge overrides:
- **Hibernate**: Total lockdown (zero tool calls, no I/O)
- **Hustle**: Rate-limited (max 5 tools, max 2 mutations)

### Anti-Flapping (Hysteresis)

Economic mode transitions use a Schmitt trigger:
- Enter threshold: 0.7 | Exit threshold: 0.3 (amplitude deadband)
- Minimum hold: 30 seconds (temporal guard)

### Actuator Coupling

Advisory, not authoritative. `AutonomicPolicyAdapter` in Arcan:
- 2-second HTTP timeout, fail-open on any error
- Most-restrictive merge with inner PolicyGatePort
- `EconomicGateHandle` for future provider-layer model selection

### Current Status (2026-03-04)

The feedback loop is **open**: Arcan and Autonomic use separate Lago journals. Events from agent ticks do not reach Autonomic's projection fold (`last_event_seq: 0`). The controller evaluates correctly but against the default initial state, so no rules fire and the output is always the permissive default profile.

**Next milestone**: Close the loop via shared journal or event forwarding (R5 Phase 2).

---

## CI Integration

### Primary Pipeline (`control-harness.yml`)
- Triggers on: push to main, all PRs
- Steps: checkout → submodules → protoc → rust toolchain → `make ci` → `make control-audit` → capture state
- Artifacts: `.life/control/state.json` uploaded for 30-day retention

### Nightly Pipeline (`control-nightly.yml`)
- Triggers on: cron (daily 04:00 UTC)
- Purpose: detect drift, entropy, stale state
- Runs full gate sequence + strict audit

### E2E Pipelines
- `cli-e2e.yml`: build all workspace binaries, exercise CLI
- `web-e2e.yml`: start server, exercise HTTP API
