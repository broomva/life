# STATE_LAYOUT_V2.md — Unified State Root (Greenfield)

## Goal

Standardize all runtime/control/persistence paths under one canonical root for AI-native orchestration.

No backward-compatibility layer is required (greenfield assumption).

## Canonical Root

`AIOS_STATE_ROOT=/home/exedev/.aios`

All stateful services must read/write under this root only.

## Target Directory Contract

```text
/home/exedev/.aios/
  control/
    policy/
    topology/
    commands/
    state/
  runtime/
    logs/
      arcan.log
      lagod.log
      autonomicd.log
    pids/
      arcan.pid
      lagod.pid
      autonomicd.pid
    sockets/
  tenants/
    <tenant_id>/
      projects/
        <project_id>/
          sessions/
            <session_id>/
              events/
              blobs/
              memory/
              checkpoints/
  artifacts/
    exports/
    reports/
  tmp/
```

## Service Mapping (V2)

- `arcan`
  - data dir: `${AIOS_STATE_ROOT}/tenants/<tenant>/projects/<project>/sessions/<session>`
  - daemon log/pid: `${AIOS_STATE_ROOT}/runtime/logs` + `${AIOS_STATE_ROOT}/runtime/pids`
- `lagod`
  - data dir: `${AIOS_STATE_ROOT}/tenants/<tenant>/projects/<project>/sessions/<session>`
  - event journal + blob store rooted there
- `autonomicd`
  - controller state: `${AIOS_STATE_ROOT}/control/state`
  - logs/pid: runtime dirs above
- control metadata (`.life/control/*`) should move to `${AIOS_STATE_ROOT}/control/*`

## Policy Rules

1. No service may write persistent state outside `AIOS_STATE_ROOT`.
2. Build artifacts (`target/`, `.target/`) are not state and must stay out of the root.
3. Runtime logs are append-only, rotated by retention policy.
4. Session directories are immutable once checkpoint is sealed.
5. All paths must be configurable via env/CLI, with `AIOS_STATE_ROOT` as default anchor.

## Disk & Retention

- Runtime logs: keep 7 days local.
- Session checkpoints: keep latest N per session (default 5).
- Artifacts/reports: retention by age + count.
- Build cache cleanup remains mandatory post-commit/push.

## Validation Checklist

- [x] `scripts/dev/up.sh` creates required runtime dirs under `AIOS_STATE_ROOT`.
- [x] `arcan` starts with explicit state root and writes no state under repo.
- [x] `lagod` uses V2 data dir and creates journal/blobs under state root.
- [x] `autonomicd` writes control state under V2 control root.
- [ ] `make web-e2e` / `make cli-e2e` pass with V2 pathing.
- [ ] backup script includes `AIOS_STATE_ROOT`.
- [ ] Railway persistence conformance: `/sessions` survives redeploy.

## Quick Start (V2)

```bash
export AIOS_STATE_ROOT=/home/exedev/.aios
export AIOS_TENANT_ID=default
export AIOS_PROJECT_ID=life
export AIOS_SESSION_ID=dev

bash scripts/dev/up.sh
bash scripts/dev/down.sh
```

## Execution Plan (initial)

1. Introduce `AIOS_STATE_ROOT` env contract in dev scripts and docs. ✅
2. Update service startup args/env for arcan/lagod/autonomicd. ✅
3. Update control/audit scripts to validate path invariants.
4. Update CI to run with explicit `AIOS_STATE_ROOT`.
5. Add lint/audit rule: fail if runtime state is written under repo working tree.
6. Implement session-index durability/rebuild so `/sessions` remains stable after redeploy (see `RAILWAY_SMOKE_REPORT_2026-03-08.md`).
