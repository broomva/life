# Railway config-as-code — substrate services

Each Life substrate service on Railway (project `Life`, environment
`production`) builds the whole Cargo workspace from a Docker build context
rooted at the repo root. The build inputs are therefore **shared** across
every substrate binary, not just that binary's own crate subtree.

## Why these files exist (BRO-1467)

The substrate services previously watched **crate subtrees only** in their
Railway dashboard `watchPatterns`. That missed two build-input roots that the
active Dockerfiles (`docker/*.Dockerfile`) copy into the build context:

- `proto/**` — read by the `*-substrate-proto`, `aios-proto`, and
  `life-*-proto` build scripts (`build.rs`) relative to the repo root.
- `apps/**` — workspace members that must be present for `cargo` to resolve
  the workspace manifest.

A change under `proto/` or `apps/` recompiles the binary but did **not** trigger
a Railway rebuild, so production drifted from source. See the arcan Dockerfile's
own comment block for the canonical statement of these inputs.

These [config-as-code](https://docs.railway.com/config-as-code) files pin
`watchPatterns` to the **full set of Docker build inputs**, so the trigger set
can never silently drift from the build context again. `watchPatterns` is the
only field declared — every other build/deploy setting continues to combine
from the Railway dashboard.

| File | Service | Extra input |
| --- | --- | --- |
| `arcan.railway.json` | `arcan` | `agents/**` (blessed authored agents copied into the runtime image) |
| `lagod.railway.json` | `lagod` | — |
| `haimad.railway.json` | `haimad` | — |
| `autonomicd.railway.json` | `autonomicd` | — |

The declared pattern set is a **superset** of the previous "crate subtrees
only" configuration, so it can only *widen* the rebuild trigger set — the worst
case is an over-rebuild, never a missed one.

## Wiring

Each service's Railway **Config-as-code file path** is set to its file above
(e.g. `deploy/railway/arcan.railway.json`), and the equivalent `watchPatterns`
were applied directly to the live service settings so the fix takes effect
before this config lands on the deployed branch. Once on the deployed branch,
the file is authoritative (config-as-code always overrides the dashboard for
the fields it declares).
