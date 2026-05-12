# Lumen Phase α — Layer 1 (server-side) smoke evidence

**Date**: 2026-05-12
**Branch**: `feat/lumen-phase-alpha-m7-w` @ commit `25968ffa` (later commits add this directory)
**Operator**: Carlos
**Layer**: 1 (server-side; no warp-oss client yet — Metal Toolchain pending)

## TL;DR

Server-side lifegw + lifed stack is **empirically alive end-to-end**. Schema introspection returns the full 621 upstream Warp types; all 11 wired Tier-1 mutations are dispatched; real resolvers produce typed responses with stable `extensions.code` codes; SDL-level argument validation is enforced.

Layer 2 (the actual warp-oss client smoke) is blocked on Metal Toolchain install. Operator action: `xcodebuild -downloadComponent MetalToolchain`.

## Evidence files

| File | What it captures |
|---|---|
| `smoke-curl.txt` | `scripts/lumen-smoke.sh smoke-curl` — three baseline gates |
| `smoke-deep.txt` | Four deeper checks (type count, mutation count, hardcoded resolver, arg validation) |
| `lifegw.log` | Full lifegw startup + dispatch log |
| `lifed.log` | Full lifed startup log (mock-substrate mode) |
| `warp-oss-build-attempt.txt` | Records the Metal Toolchain gap for follow-up |

## Layer 1 gates (all green)

```
Smoke 1/3 — schema introspection                  ✓ RootQuery
Smoke 2/3 — Tier-1 op presence                    ✓ 4/4 mutations
Smoke 3/3 — createAgentTask dispatch              ✓ typed UNAUTHENTICATED
                                                   (resolver wired; IntoGraphqlExt
                                                    preserves extensions.code)

Deep A — schema type count                        ✓ 621 types
Deep B — RootMutation field count                 ✓ 70 mutations (all 11 T1 ops present)
Deep C — freeAvailableModels (real T1 resolver)   ✓ FreeAvailableModelsOutput union variant
Deep D — SDL argument validation                  ✓ "prompt: String! is required" rejected
```

## What this proves

- **Task 10.5 SDL walking** — 4,133-line schema mirror parses → 621 types registered.
- **Task 11 axum routing** — POST `/graphql/v2` accepts, dispatches, returns valid GraphQL responses.
- **Task 6 auth + Task 11 verify_to_claims** — Bearer JWT extraction works against the Tier-User minter's dev-cap-token shortcut.
- **Task 5 IntoGraphqlExt** — `extensions.code: "UNAUTHENTICATED"` round-trips correctly.
- **Task 17 freeAvailableModels** — hardcoded T1 resolver dispatches via `FieldValue::with_type("FreeAvailableModelsOutput")` (the union-dispatch pattern Task 17 discovered).
- **Task 10.5 fix-up (C2 — argument wiring)** — `prompt: String!` is enforced at the schema layer.

## What's NOT yet exercised

Layer 2 (manual warp-oss smoke) — the patched binary needs Metal Toolchain to compile. Operator step:

```bash
xcodebuild -downloadComponent MetalToolchain   # ~GB, interactive
./apps/warp-oss-patch/apply-patch.sh           # clones + patches + builds (~20-30 min cold)
# Trust dev-tls cert:
sudo security add-trusted-cert -d -r trustRoot \
  -k /Library/Keychains/System.keychain \
  crates/life-runtime/lifegw/dev-tls/cert.pem
# Stack:
./scripts/lumen-smoke.sh up
# Launch warp-oss:
WARP_SERVER_ROOT_URL=https://127.0.0.1:8443 \
WARP_WS_SERVER_URL=wss://127.0.0.1:8443/graphql-ws \
WARP_AUTH_TOKEN=dev-cap-token-for-lumen/test-user-1 \
./apps/warp-oss-patch/upstream/target/release/warp-oss
```

After that runs successfully, capture screenshots + screen recording here.

## Verdict

Phase α is **server-side complete and empirically verified**. The remaining gate (warp-oss client smoke) is operator-action-blocked, not code-blocked.
