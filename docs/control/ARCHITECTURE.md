# Control-Aware Architecture

**Last updated**: 2026-03-03

System design for the control plane governing the `/live` monorepo.

---

## Components

```
.control/                          Declarative policy, commands, topology
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
- Control state persisted in `.control/state.json` (audit timestamps, controller mode).

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
| `spaces/` | Networking | HIGH | fmt + clippy + check |
| `docs/` | Shared | MEDIUM | Existence audit |
| `.control/` | Control plane | HIGH | Strict audit |
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

1. **aiOS** must not depend on Arcan or Lago implementation crates.
2. **Lago core** may only depend on `aios-protocol` (not Arcan crates).
3. **Arcan** may depend on aiOS and Lago through adapter boundaries only.
4. **No circular dependencies** between project workspaces.

Violations cause `make audit` to fail, blocking CI.

---

## CI Integration

### Primary Pipeline (`control-harness.yml`)
- Triggers on: push to main, all PRs
- Steps: checkout → submodules → protoc → rust toolchain → `make ci` → `make control-audit` → capture state
- Artifacts: `.control/state.json` uploaded for 30-day retention

### Nightly Pipeline (`control-nightly.yml`)
- Triggers on: cron (daily 04:00 UTC)
- Purpose: detect drift, entropy, stale state
- Runs full gate sequence + strict audit

### E2E Pipelines
- `cli-e2e.yml`: build all workspace binaries, exercise CLI
- `web-e2e.yml`: start server, exercise HTTP API
