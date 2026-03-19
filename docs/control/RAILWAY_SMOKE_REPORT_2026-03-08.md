# Railway Smoke Report — 2026-03-08

## Scope

Validate cloud deployment and service wiring for `life` stack on Railway:
- `lagod`
- `autonomicd`
- `arcan`

Target objective: verify service health, inter-service networking, and persistence behavior.

## Environment

- Project: `life-smoke`
- Services: `lagod`, `autonomicd`, `arcan`
- Runtime approach: service-specific Dockerfiles under `deploy/railway/`
- Unified state root env: `AIOS_STATE_ROOT=/data/.aios`

## What was deployed

### Deploy assets added
- `.railwayignore`
- `deploy/railway/Dockerfile.lagod`
- `deploy/railway/Dockerfile.autonomicd`
- `deploy/railway/Dockerfile.arcan`

### Runtime wiring
- `arcan` configured with:
  - `ARCAN_AUTONOMIC_URL=http://autonomicd.railway.internal:3002`
  - `ARCAN_BIND=0.0.0.0`
  - `--port ${PORT}` (Railway-assigned)

### Volumes
- `lagod` volume mounted at `/data`
- `arcan` volume mounted at `/data`

## Findings

## 1) Service health
- `lagod`: healthy (`/health` -> 200)
- `autonomicd`: healthy (`/health` -> 200)
- `arcan`: healthy (`/health` -> 200) after bind fix

### Root cause resolved
`arcan` initially failed ingress due to listener binding on `127.0.0.1`. Added `ARCAN_BIND` support in runtime and set to `0.0.0.0` in Railway env.

## 2) Internal network wiring
- `arcan` logs confirm remote autonomic advisory enabled via internal hostname:
  - `http://autonomicd.railway.internal:3002`
- Cross-service startup path is operational.

## 3) Functional E2E API checks
- `POST /sessions` successful
- `POST /sessions/{id}/runs` successful
- `GET /sessions` returned active sessions within runtime window
- SSE stream endpoint emitted session/run lifecycle events

## 4) Persistence behavior across redeploy (critical)
Observed issue:
- after service redeploy, `/sessions` list returned empty in subsequent checks,
  despite `/data/.aios/...` pathing and mounted volumes.

Interpretation:
- infrastructure persistence is present (volumes attached),
- but session index visibility is not durable/reconstructed as expected after restart.

Likely app-layer gap:
- session listing relies on in-memory registry or non-rebuilt index,
- startup path does not rehydrate `/sessions` view from durable journal/source-of-truth.

## Current verdict

- **Wiring:** PASS
- **Health:** PASS
- **Functional smoke:** PASS
- **Durable session listing after redeploy:** FAIL (app-layer persistence semantics)

## Required next fixes

1. Implement durable session index semantics in `arcan`:
   - persist session manifest/index, or
   - rebuild session index from journal on startup.
2. Add automated persistence conformance test:
   - create session+run,
   - redeploy,
   - assert session still appears in `/sessions`.
3. Add explicit startup log for index rehydration result (count, source, duration).

## Follow-up acceptance criteria

- [ ] `GET /sessions` survives arcan+lagod redeploy with previously created session IDs.
- [ ] run replay/state reconstruction works for persisted sessions.
- [ ] CI/preview pipeline includes persistence-resilience smoke test.
