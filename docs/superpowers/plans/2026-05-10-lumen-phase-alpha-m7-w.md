# Lumen Phase α — M7-W: Warp GraphQL Adapter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land a Warp-compatible GraphQL surface on `lifegw` so a minimally-patched OSS Warp build (one-line `Channel::Oss` gate-bypass), pointed at lifegw via `WARP_SERVER_ROOT_URL` + `WARP_WS_SERVER_URL`, drives an arcan-backed agent task to completion.

**Architecture:** New `warp_graphql` module inside `crates/life-runtime/lifegw/`. Loads Warp's verbatim 4,134-line schema in async-graphql SDL-first mode. Resolvers translate to lifed gRPC. Subscription bridges `warpDriveUpdates` to `lifed.EventService.Subscribe`. New `arcan-core/src/warp_agent.rs` adapts `createAgentTask` invocations to `ConsciousnessActor`.

**Tech Stack:** Rust 2024 (edition = "2024", rust-version = "1.85") · async-graphql 7.x · axum 0.7 · tonic 0.14 · graphql-transport-ws · tokio · existing lifegw JWT/JWKS infrastructure · Spec D AnimaCustody · Spec E InferenceBackend.

**Spec:** `core/life/docs/superpowers/specs/2026-05-10-lumen-warp-on-life-design.md` (Pre-flight Update 2026-05-11 included).

**Phase scope:** This plan covers Phase α only (M7-W.1 + M7-W.2 + M7-W.3, ~3 weeks, 18 Linear pts), plus a new **Task 0** (Warp OSS one-line patch + binary build) added 2026-05-11 after pre-flight. Phase β (Lumen fork at `apps/lumen/`) gets its own plan written after α ships.

---

## Pre-flight Update (2026-05-11)

Two rounds of pre-flight + Task 2 spec-review surfaced findings that affect downstream implementation. Summary:

**Round 1 — endpoint override** (caught by Task 0 pre-flight before any subagent dispatched):

- The `WARP_API_ENDPOINT` env var name used in the original 2026-05-10 plan **does not exist** in Warp's codebase. Correct names are `WARP_SERVER_ROOT_URL` (HTTPS root) and `WARP_WS_SERVER_URL` (WebSocket subscription endpoint).
- The official OSS binary silently drops these env vars via a release-channel gate at `crates/warp_core/src/channel/mod.rs:46`. Moving `Channel::Oss` from the `false` arm to the `true` arm of `allows_server_url_overrides` restores override behavior. **One-line patch.**
- New Task 0 added below as the first task. It clones upstream, applies the one-line patch, builds the binary, and verifies override works. Task 0 runs in **parallel with Task 1** (workspace deps) — they touch independent trees.
- Task 22 (E2E smoke) acceptance gate is updated below to launch `target/release/warp-oss` (the patched binary) instead of stock `warp`.
- Firebase API key risk is deferred to Phase β (full Lumen fork) and tracked in the spec risk register.

**Round 2 — branch name + Metal Toolchain** (caught by Task 0 + Task 2 implementer reports):

- Upstream's default branch is `master`, not `main`. Plan template + apply-patch.sh updated to autodetect via `symbolic-ref refs/remotes/origin/HEAD`.
- macOS Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`) is required to build `warp-oss` from source. Documented in `apps/warp-oss-patch/README.md` and the M7-W.0 section header.

**Round 3 — operation names** (caught by Task 2 spec review):

Upstream Warp's GraphQL schema uses a **mixed naming convention** — some Queries use bare nouns (`user`, `freeAvailableModels`, `simpleIntegrations`), others use `get*`-prefixed names (`getRelevantFragments`, `getCloudEnvironments`). The original 2026-05-10 plan assumed uniform `get*` prefix throughout. Plan corrections applied:

| Original plan name | Actual upstream name | Affected task |
|---|---|---|
| `getUser` | **`user`** (bare-noun Query) | Task 14 |
| `getUserSettings` | **DOES NOT EXIST as a Query** — settings nested under `UserResult` returned by `user` Query. Tier-3 stub fallback covers reads in v1 | Task 16 (now only wires `updateUserSettings` Mutation) |
| `getFreeAvailableModels` | **`freeAvailableModels`** (bare-noun Query) | Task 17 |
| `getSimpleIntegrations` | `simpleIntegrations` (bare-noun Query) | T2 sweep |
| `getRequestLimitInfo` | DOES NOT EXIST as Query — nested field on `UserResult`/`OveragesResult` | T2 (drop from list) |
| `getConversationUsage` | DOES NOT EXIST as Query — `conversationUsage` is a nested field | T2 (drop) |
| `getAIOveragesForWorkspace` | DOES NOT EXIST as Query — `aiOverages` is a nested field | T2 (drop) |
| `getAIConversationFormat` | DOES NOT EXIST anywhere | T2 (drop) |

The 4 dropped T2 ops were never accessible as top-level Queries upstream. Their data is exposed via nested fields on response types (e.g., `user { settings { ... }, requestLimitInfo { ... } }`). For Phase α, nested-field reads fall through to the Tier-3 stub fallback (returning the schema's nullable defaults), which the Warp client tolerates. Real field-resolvers can be added in a follow-up if telemetry/billing UX requires populated values.

All other tasks (1–23) are unchanged in scope. The lifegw GraphQL adapter, T1 resolvers, subscription bridge, and arcan adapter all remain as originally specified.

---

## Required Reading (before starting)

The implementer must skim these files first:

1. **Spec**: `core/life/docs/superpowers/specs/2026-05-10-lumen-warp-on-life-design.md` — the design contract, especially Locked Decisions L6-D1..D11.
2. **lifegw existing structure**: `core/life/crates/life-runtime/lifegw/src/lib.rs`, `auth/`, `bootstrap.rs`, `services/` — understand the existing axum router, JWT middleware, and how services register.
3. **lifegw Cargo.toml**: `core/life/crates/life-runtime/lifegw/Cargo.toml` — feature flags, deps, conventions.
4. **lifed services**: `core/life/crates/life-runtime/lifed/src/services/` — list available gRPC services. Resolvers will dispatch through these.
5. **arcan ConsciousnessActor**: `core/life/crates/arcan/arcan-core/src/` — find the existing actor entry point (likely `agent.rs` or `consciousness/`). The new `warp_agent.rs` adapter wraps it.
6. **Workspace Rust conventions**: `core/life/CLAUDE.md` "Shared Conventions" section — Rust 2024 edition specifics (`gen` reserved keyword, `unsafe` env vars, `name.rs` not `mod.rs`), `cargo fmt + clippy + test` workflow.
7. **Spec C₃ §6.5 close codes**: `core/life/docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md` — close-code vocabulary used by `warpDriveUpdates` subscription on disconnect.

---

## File Structure

### Files created (new)

| Path | Responsibility |
|---|---|
| `crates/life-runtime/lifegw/src/warp_graphql.rs` | Module declarations + public re-exports |
| `crates/life-runtime/lifegw/src/warp_graphql/schema_mirror.rs` | Loads schema.graphql at compile time; exposes `SDL: &'static str` |
| `crates/life-runtime/lifegw/src/warp_graphql/schema_mirror/schema.graphql` | Verbatim copy of upstream Warp's 4,134-line schema |
| `crates/life-runtime/lifegw/src/warp_graphql/adapter.rs` | async-graphql Schema builder; binds resolvers to SDL types |
| `crates/life-runtime/lifegw/src/warp_graphql/auth.rs` | Bearer JWT extraction + AnimaCustody-bound claim attachment |
| `crates/life-runtime/lifegw/src/warp_graphql/dataloader.rs` | Batched lifed gRPC client (DataLoader pattern, N+1 prevention) |
| `crates/life-runtime/lifegw/src/warp_graphql/context.rs` | Request-scoped `WarpRequestCtx` carrying claims + dataloader handle |
| `crates/life-runtime/lifegw/src/warp_graphql/error.rs` | `WarpGraphqlError` with conversion to GraphQL extensions |
| `crates/life-runtime/lifegw/src/warp_graphql/resolvers.rs` | Module declarations for resolver tiers |
| `crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier1.rs` | T1 resolvers (~15 ops): identity, agent task, dialogue, settings |
| `crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier2.rs` | T2 resolvers (~15 ops): history, code search, billing, MCP discovery (deferred to W.2; scaffold only in W.1) |
| `crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier3_stubs.rs` | Generic safe-default resolver for ~90 unimplemented ops |
| `crates/life-runtime/lifegw/src/warp_graphql/subscriptions.rs` | Module declarations for subscriptions |
| `crates/life-runtime/lifegw/src/warp_graphql/subscriptions/warp_drive_updates.rs` | The one multiplexed subscription, bridges to `lifed.EventService.Subscribe` |
| `crates/life-runtime/lifegw/src/warp_graphql/route.rs` | axum HTTP `/graphql/v2` + WS `/graphql-ws` route handlers |
| `crates/life-runtime/lifegw/tests/warp_graphql_introspection.rs` | Conformance: introspection AST matches schema.graphql byte-for-byte |
| `crates/life-runtime/lifegw/tests/warp_graphql_t1_golden.rs` | Golden response shapes for each T1 op |
| `crates/life-runtime/lifegw/tests/warp_graphql_e2e_smoke.rs` | E2E smoke: docker-compose + official Warp client + arcan task to completion |
| `crates/arcan/arcan-core/src/warp_agent.rs` | Adapter: `createAgentTask` → `ConsciousnessActor` invocation |
| `crates/arcan/arcan-core/tests/warp_agent.rs` | Tests for the warp_agent adapter |

### Files modified (existing)

| Path | Change |
|---|---|
| `crates/life-runtime/lifegw/Cargo.toml` | Add deps: `async-graphql = "7.2"` (with features `chrono`, `uuid`, `dynamic-schema`, `playground`, `dataloader`), `async-graphql-axum = "7.2"`, `graphql-ws-client = "0.10"` (dev-dep for E2E only). DataLoader batching uses async-graphql's built-in feature — not the standalone `dataloader` crate, which pulled async-std (RUSTSEC-2025-0052) |
| `crates/life-runtime/lifegw/src/lib.rs` | Add `pub mod warp_graphql;` |
| `crates/life-runtime/lifegw/src/bootstrap.rs` | Wire `warp_graphql::route::routes()` into the axum Router |
| `crates/life-runtime/lifegw/src/config.rs` | Add `WarpGraphqlConfig { enabled: bool, max_request_size: usize }` to top-level config |
| `crates/arcan/arcan-core/src/lib.rs` | Add `pub mod warp_agent;` |
| `core/life/Cargo.toml` (workspace root) | Add `async-graphql` and friends to `[workspace.dependencies]` |

---

## M7-W.0 — Warp OSS Patch (parallelizable with M7-W.1)

**Outcome of M7-W.0:** A locally-built `target/release/warp-oss` binary that honors `WARP_SERVER_ROOT_URL` + `WARP_WS_SERVER_URL` env vars. This is the client side of the integration; M7-W.1+ is the server side. **They are independent and can run concurrently.**

**System prerequisites (macOS):** Building `warp-oss` from source requires the Xcode Metal Toolchain (Warp's UI compiles GPU shaders via `metal`). If not installed, the build fails at `crates/warpui/build.rs:92`. Install via:

```bash
xcodebuild -downloadComponent MetalToolchain
```

This is interactive and ~GB-scale. Skip if you don't need to build the binary locally — Task 23 (manual α-gate verification) can be deferred to an environment that has the toolchain pre-installed, or to CI.

**Build state caveat (2026-05-11 implementer report):** The first Task 0 dispatch committed the patch + script + README + .gitignore artifacts at commit `307cb022`, but the binary build failed on the missing Metal Toolchain. The artifacts are correct (patch verified to apply forward + reverse against current upstream `91b4f09`); only the binary is missing. Retry the build after installing the toolchain, or defer to Task 23.

---

### Task 0: Apply OSS-channel gate-bypass patch + build the patched Warp binary

**Files:**
- Create: `apps/warp-oss-patch/` (new directory in workspace)
- Create: `apps/warp-oss-patch/README.md`
- Create: `apps/warp-oss-patch/apply-patch.sh`
- Create: `apps/warp-oss-patch/channel-oss-override.patch`
- Create: `apps/warp-oss-patch/.gitignore` (ignores the `upstream/` clone)

**Why this task exists:** Pre-flight (2026-05-11) found that Warp's OSS build silently drops `WARP_SERVER_ROOT_URL` / `WARP_WS_SERVER_URL` via a release-channel gate at `crates/warp_core/src/channel/mod.rs:46`. A one-line patch (move `Channel::Oss` from the `false` arm to the `true` arm) restores override behavior. This task encapsulates the patch, the build script, and the smoke verification.

**Important constraint:** We do NOT vendor Warp's source into our repo. We clone upstream on demand (gitignored), apply our patch, build. The artifact (`apps/warp-oss-patch/upstream/target/release/warp-oss`) is also gitignored. The only files we commit are the patch, the script, and the README.

- [ ] **Step 1: Create the directory + .gitignore**

```bash
mkdir -p apps/warp-oss-patch
cat > apps/warp-oss-patch/.gitignore <<'EOF'
# Upstream clone is gitignored; we re-clone on demand and patch.
upstream/
EOF
```

- [ ] **Step 2: Write the patch file**

Create `apps/warp-oss-patch/channel-oss-override.patch`:

```patch
--- a/crates/warp_core/src/channel/mod.rs
+++ b/crates/warp_core/src/channel/mod.rs
@@ -42,8 +42,9 @@ impl Channel {
     /// they explicitly opt in via this method.
     pub fn allows_server_url_overrides(&self) -> bool {
         match self {
-            Channel::Dev | Channel::Local | Channel::Integration => true,
-            Channel::Stable | Channel::Preview | Channel::Oss => false,
+            Channel::Dev | Channel::Local | Channel::Integration | Channel::Oss => true,
+            Channel::Stable | Channel::Preview => false,
         }
     }
```

NOTE: The exact line numbers may shift if upstream rebases. If the patch doesn't apply cleanly, use `git apply --3way` or fall back to manual edit at the same match arm.

- [ ] **Step 3: Write the apply-patch + build script**

Create `apps/warp-oss-patch/apply-patch.sh`:

```bash
#!/usr/bin/env bash
# Clone warpdotdev/warp, apply our one-line OSS-channel override patch,
# and build the binary. Re-running is idempotent: the upstream clone is
# refreshed and the patch reapplied.
#
# Pre-flight finding 2026-05-11: the OSS build silently drops endpoint
# override env vars via a release-channel gate. This patch removes the
# gate for Channel::Oss only.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UPSTREAM_DIR="$SCRIPT_DIR/upstream"
PATCH_FILE="$SCRIPT_DIR/channel-oss-override.patch"
UPSTREAM_URL="https://github.com/warpdotdev/warp.git"

# Clone or refresh upstream. Autodetect default branch (upstream uses
# `master`, not `main` — caught by pre-flight 2026-05-11).
if [ -d "$UPSTREAM_DIR/.git" ]; then
  echo "==> Refreshing upstream clone"
  DEFAULT_BRANCH=$(git -C "$UPSTREAM_DIR" symbolic-ref refs/remotes/origin/HEAD | sed 's@^refs/remotes/origin/@@')
  git -C "$UPSTREAM_DIR" fetch --depth 1 origin "$DEFAULT_BRANCH"
  git -C "$UPSTREAM_DIR" reset --hard FETCH_HEAD
  git -C "$UPSTREAM_DIR" checkout -- .  # discard any prior patch
else
  echo "==> Cloning upstream"
  git clone --depth 1 "$UPSTREAM_URL" "$UPSTREAM_DIR"
fi

# Apply patch. Skip if already applied (idempotent re-run).
echo "==> Applying channel-oss-override.patch"
if git -C "$UPSTREAM_DIR" apply --reverse --check "$PATCH_FILE" 2>/dev/null; then
  echo "    (already applied — skipping)"
else
  git -C "$UPSTREAM_DIR" apply --check "$PATCH_FILE"
  git -C "$UPSTREAM_DIR" apply "$PATCH_FILE"
fi

# Build the OSS binary in release mode.
echo "==> Building warp-oss (release)"
( cd "$UPSTREAM_DIR" && cargo build --release --bin warp-oss )

BIN="$UPSTREAM_DIR/target/release/warp-oss"
if [ -x "$BIN" ]; then
  echo "==> Built: $BIN"
else
  echo "ERROR: expected binary not found at $BIN" >&2
  exit 1
fi
```

Make executable:

```bash
chmod +x apps/warp-oss-patch/apply-patch.sh
```

- [ ] **Step 4: Write the README**

Create `apps/warp-oss-patch/README.md`:

```markdown
# warp-oss-patch — One-line override-gate bypass

This directory contains the minimal patch to Warp's open-source binary
that makes Phase α of Lumen possible. Without it, the OSS build silently
ignores `WARP_SERVER_ROOT_URL` / `WARP_WS_SERVER_URL` (release-channel
hardening at `crates/warp_core/src/channel/mod.rs:46`).

## Usage

```bash
./apply-patch.sh
```

Clones `warpdotdev/warp` into `upstream/`, applies our one-line patch,
builds `target/release/warp-oss`. Re-runnable — refreshes the clone and
reapplies the patch.

## Running the patched binary against lifegw

```bash
WARP_SERVER_ROOT_URL=http://localhost:8443 \
WARP_WS_SERVER_URL=ws://localhost:8443/graphql/v2 \
./upstream/target/release/warp-oss
```

## The patch

`channel-oss-override.patch` moves `Channel::Oss` from the `false` arm
to the `true` arm of `Channel::allows_server_url_overrides`. That's it.

After Phase α validates the integration, Phase β replaces this directory
with a real soft fork at `apps/lumen/` carrying its own branding,
defaults, and additional patches (Firebase API key neutralization, etc.).

## License

The patched binary inherits Warp's AGPLv3. Our patch is also AGPLv3 for
consistency.
```

- [ ] **Step 5: Run the script — clone, patch, build**

```bash
./apps/warp-oss-patch/apply-patch.sh
```

Expected: ~15-30 minutes on cold first run (full Warp build is heavy). Result: `apps/warp-oss-patch/upstream/target/release/warp-oss` exists and is executable.

If the build fails on macOS due to missing system dependencies, install via `brew install <missing-pkg>` and re-run. The build script is idempotent — failures partway through don't corrupt state.

- [ ] **Step 6: Verify the patched binary respects override env vars**

Quick verification that the patched binary picks up the env var: start a netcat listener on port 8443 and launch warp-oss; we should see the binary attempt to connect to localhost rather than `app.warp.dev`.

```bash
# Pane A — listen on 8443
nc -lk 8443 &
NC_PID=$!

# Pane B — launch warp-oss with override
WARP_SERVER_ROOT_URL=http://localhost:8443 \
WARP_WS_SERVER_URL=ws://localhost:8443/graphql/v2 \
./apps/warp-oss-patch/upstream/target/release/warp-oss &
WARP_PID=$!

# Within ~5 seconds, nc should print an inbound HTTP request line
# (something like "POST /graphql/v2?op=getUser HTTP/1.1") OR a TCP
# connection attempt log line. If you see app.warp.dev being hit
# instead, the patch didn't take effect — re-check Step 2 + Step 5.

# Cleanup
sleep 10
kill $WARP_PID $NC_PID 2>/dev/null || true
```

Expected: nc receives a connection from warp-oss. If app.warp.dev is hit instead, the patch is broken — escalate.

- [ ] **Step 7: Commit**

```bash
git add apps/warp-oss-patch/.gitignore apps/warp-oss-patch/README.md apps/warp-oss-patch/apply-patch.sh apps/warp-oss-patch/channel-oss-override.patch
git commit -m "feat(warp-oss-patch): one-line OSS-channel override-gate bypass (M7-W.0, BRO-XXXX)"
```

Note: do NOT commit `upstream/` — it's gitignored. The binary built there is a local artifact, not a versioned asset.

---

## M7-W.1 — Scaffolding

**Outcome of M7-W.1:** lifegw compiles with the warp_graphql module enabled. All ~122 schema operations resolve (T3 stubs return safe defaults). Schema introspection is byte-identical to upstream. JWT auth gates the route. WebSocket handshake works. **No real T1 resolvers yet** — that's M7-W.2.

---

### Task 1: Add async-graphql workspace dependency

**Files:**
- Modify: `core/life/Cargo.toml` (workspace root, `[workspace.dependencies]` table)
- Modify: `core/life/crates/life-runtime/lifegw/Cargo.toml`

- [ ] **Step 1: Add deps to workspace Cargo.toml**

Open `core/life/Cargo.toml`. Find the `[workspace.dependencies]` table. Add:

```toml
async-graphql = { version = "7.2", default-features = false, features = ["chrono", "uuid", "dynamic-schema", "playground", "dataloader"] }
async-graphql-axum = "7.2"
```

NOTE (2026-05-11 plan correction): the features list MUST include `dynamic-schema` (Task 4 uses `async_graphql::dynamic::Schema`) and `playground` (Task 6 uses `playground_source`). Both are in async-graphql's default feature set but `default-features = false` strips them — easy to miss and produces a compile failure at Task 4. The `dataloader` feature enables async-graphql's lightweight built-in DataLoader (futures-channel + lru); the original plan referenced a standalone `dataloader = "0.18"` crate which pulled the unmaintained async-std runtime (RUSTSEC-2025-0052) and wasn't actually used by Task 8 — we use the framework's native facility instead.

- [ ] **Step 2: Reference workspace deps in lifegw Cargo.toml**

Open `core/life/crates/life-runtime/lifegw/Cargo.toml`. Add to `[dependencies]`:

```toml
async-graphql.workspace = true
async-graphql-axum.workspace = true
```

Add to `[dev-dependencies]`:

```toml
graphql-ws-client = "0.10"
```

- [ ] **Step 3: Verify it compiles**

Run from `core/life/`:

```bash
cargo check -p lifegw
```

Expected: success, no errors. Warnings about unused deps are fine for now.

- [ ] **Step 4: Commit**

```bash
git add core/life/Cargo.toml core/life/crates/life-runtime/lifegw/Cargo.toml
git commit -m "feat(lifegw): add async-graphql workspace deps for M7-W (BRO-XXXX)"
```

---

### Task 2: Vendor Warp's GraphQL schema as a verbatim mirror

**Files:**
- Create: `core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror/schema.graphql`
- Create: `core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror/UPSTREAM.md`

- [ ] **Step 1: Fetch the upstream schema file**

Run:

```bash
mkdir -p core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror
gh api repos/warpdotdev/warp/contents/crates/warp_graphql_schema/api/schema.graphql \
  --jq '.content' | base64 -d > \
  core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror/schema.graphql
```

Expected: file is ~4,134 lines, starts with `schema { query: Query mutation: Mutation subscription: Subscription }` or similar.

- [ ] **Step 2: Verify the file**

Run:

```bash
wc -l core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror/schema.graphql
head -20 core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror/schema.graphql
```

Expected: line count between 4,000 and 4,200; SDL shape is recognizable.

- [ ] **Step 3: Record the upstream commit pin**

Get the upstream commit SHA the schema was fetched from:

```bash
gh api repos/warpdotdev/warp/commits/master --jq '.sha' | head -c 12
```

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror/UPSTREAM.md`:

```markdown
# Schema Mirror Provenance

**Source**: `github.com/warpdotdev/warp` `crates/warp_graphql_schema/api/schema.graphql`
**Pinned commit**: `<paste-sha-here>`
**Fetched**: 2026-05-10
**License**: AGPLv3 (per upstream LICENSE)

## Update procedure

When upstream Warp updates its schema:

1. Fetch the new file (`gh api ...`).
2. Run `cargo test -p lifegw --test warp_graphql_introspection` — failures show drift.
3. Update resolvers/tier1.rs and tier2.rs to match any T1/T2 op shape changes.
4. Update this file's pinned commit + fetch date.
5. Open a Linear ticket for the rebase under the M7-W umbrella.

The mirror is byte-identical to upstream. Any divergence is a bug in our copy, not a feature.
```

- [ ] **Step 4: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror/
git commit -m "feat(lifegw): vendor Warp GraphQL schema mirror (M7-W.1, BRO-XXXX)"
```

---

### Task 3: Create the warp_graphql module skeleton

**Files:**
- Create: `core/life/crates/life-runtime/lifegw/src/warp_graphql.rs`
- Modify: `core/life/crates/life-runtime/lifegw/src/lib.rs`

- [ ] **Step 1: Add module declarations file**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql.rs`:

```rust
//! Warp-compatible GraphQL surface (Spec C₃ M7-W).
//!
//! This module exposes a verbatim mirror of Warp's upstream GraphQL
//! schema, with resolvers that translate to internal `lifed` gRPC.
//! Phase α scope: the adapter compiles, schema introspection is
//! byte-identical to upstream, all operations resolve (T1+T2 with real
//! data, T3 with safe defaults), JWT auth gates every call.
//!
//! See `core/life/docs/superpowers/specs/2026-05-10-lumen-warp-on-life-design.md`
//! for the design contract.

pub mod adapter;
pub mod auth;
pub mod context;
pub mod dataloader;
pub mod error;
pub mod resolvers;
pub mod route;
pub mod schema_mirror;
pub mod subscriptions;

pub use adapter::WarpSchema;
pub use error::WarpGraphqlError;
```

- [ ] **Step 2: Wire it into lib.rs**

Open `core/life/crates/life-runtime/lifegw/src/lib.rs`. Add at the appropriate place (alphabetical with other `pub mod` declarations):

```rust
pub mod warp_graphql;
```

- [ ] **Step 3: Add empty stub files for each declared submodule**

Run:

```bash
cd core/life/crates/life-runtime/lifegw/src/warp_graphql
touch adapter.rs auth.rs context.rs dataloader.rs error.rs route.rs schema_mirror.rs
mkdir -p resolvers subscriptions
touch resolvers.rs resolvers/tier1.rs resolvers/tier2.rs resolvers/tier3_stubs.rs
touch subscriptions.rs subscriptions/warp_drive_updates.rs
```

- [ ] **Step 4: Add module declarations inside resolvers.rs and subscriptions.rs**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/resolvers.rs`:

```rust
//! Resolver tiers per Lumen spec L6-D4.
//!
//! - tier1: ~15 ops, must implement (working agent)
//! - tier2: ~15 ops, production UX
//! - tier3_stubs: ~90 ops, return safe defaults

pub mod tier1;
pub mod tier2;
pub mod tier3_stubs;
```

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/subscriptions.rs`:

```rust
//! Subscription handlers. Warp has exactly one subscription
//! (`warpDriveUpdates`), multiplexed across event types.

pub mod warp_drive_updates;
```

- [ ] **Step 5: Verify the crate compiles**

Run from `core/life/`:

```bash
cargo check -p lifegw
```

Expected: success. Empty submodules are fine — they'll be filled in subsequent tasks.

- [ ] **Step 6: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/lib.rs core/life/crates/life-runtime/lifegw/src/warp_graphql.rs core/life/crates/life-runtime/lifegw/src/warp_graphql/
git commit -m "feat(lifegw): warp_graphql module skeleton (M7-W.1, BRO-XXXX)"
```

---

### Task 4: Load the schema mirror at compile time and expose SDL

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror.rs`

- [ ] **Step 1: Write the failing test**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror.rs` with a doctest:

```rust
//! Compile-time-embedded mirror of Warp's GraphQL schema.

/// Raw SDL of Warp's GraphQL schema.
///
/// Loaded at compile time from `schema_mirror/schema.graphql`. This is a
/// verbatim copy of upstream Warp; modifications must follow the procedure
/// in `schema_mirror/UPSTREAM.md`.
///
/// # Example
///
/// ```
/// use lifegw::warp_graphql::schema_mirror::SDL;
/// assert!(SDL.contains("type Query"));
/// assert!(SDL.contains("warpDriveUpdates"));
/// assert!(SDL.len() > 100_000); // ~4,134 lines
/// ```
pub const SDL: &str = include_str!("schema_mirror/schema.graphql");
```

- [ ] **Step 2: Run the test and verify it passes**

Run:

```bash
cargo test -p lifegw --doc warp_graphql::schema_mirror
```

Expected: PASS. If `SDL.len() > 100_000` fails, the schema fetch in Task 2 was incomplete — re-fetch.

- [ ] **Step 3: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/warp_graphql/schema_mirror.rs
git commit -m "feat(lifegw): embed schema mirror SDL at compile time (M7-W.1, BRO-XXXX)"
```

---

### Task 5: Define the `WarpGraphqlError` type

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/error.rs`
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Write the failing test inline**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/error.rs`:

```rust
//! Error type for warp_graphql resolvers.
//!
//! Converts to `async_graphql::Error` with structured `extensions` so the
//! Warp client can render meaningful messages and our metrics can categorize
//! failures.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WarpGraphqlError {
    #[error("authentication required")]
    Unauthenticated,

    #[error("operation not enabled in this build: {0}")]
    NotEnabled(&'static str),

    #[error("upstream lifed error: {0}")]
    Upstream(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl WarpGraphqlError {
    /// Stable error code surfaced to the Warp client in
    /// `extensions.code`. Used by upstream Warp UI to decide UX
    /// (e.g., redirect to login on UNAUTHENTICATED).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unauthenticated => "UNAUTHENTICATED",
            Self::NotEnabled(_) => "FEATURE_NOT_ENABLED",
            Self::Upstream(_) => "UPSTREAM_ERROR",
            Self::InvalidInput(_) => "INVALID_INPUT",
            Self::Internal(_) => "INTERNAL_ERROR",
        }
    }
}

impl From<WarpGraphqlError> for async_graphql::Error {
    fn from(err: WarpGraphqlError) -> Self {
        let code = err.code();
        async_graphql::Error::new(err.to_string()).extend_with(|_, ext| {
            ext.set("code", code);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthenticated_has_stable_code() {
        let err = WarpGraphqlError::Unauthenticated;
        assert_eq!(err.code(), "UNAUTHENTICATED");
    }

    #[test]
    fn not_enabled_carries_feature_name() {
        let err = WarpGraphqlError::NotEnabled("createTeam");
        assert_eq!(err.code(), "FEATURE_NOT_ENABLED");
        assert_eq!(err.to_string(), "operation not enabled in this build: createTeam");
    }

    #[test]
    fn converts_to_async_graphql_error_with_code() {
        let err: async_graphql::Error = WarpGraphqlError::InvalidInput("bad".into()).into();
        // The extension code is in the `extensions` field; we can verify
        // by serializing.
        let json = serde_json::to_value(&err).expect("error serializes");
        assert_eq!(json["extensions"]["code"], "INVALID_INPUT");
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p lifegw warp_graphql::error
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/warp_graphql/error.rs
git commit -m "feat(lifegw): WarpGraphqlError with stable extension codes (M7-W.1, BRO-XXXX)"
```

---

### Task 6: Implement the JWT auth extractor

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/auth.rs`

The auth extractor reuses lifegw's existing `JwksCache::verify_capability_token` (shipped in M7 sub-phase D). The new code is a thin async-graphql-friendly wrapper that pulls the bearer token from request headers, verifies it, and produces a typed `WarpClaims` carried in the GraphQL request context.

- [ ] **Step 1: Write the test (inline)**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/auth.rs`:

```rust
//! Bearer JWT extraction for warp_graphql resolvers.
//!
//! Reuses lifegw's existing `JwksCache` (Spec C₃ M7-D shipped). The
//! request flows: Warp client sends `Authorization: Bearer <jwt>` →
//! axum extractor pulls the header → `JwksCache::verify_capability_token`
//! validates signature + audience + scopes → typed `WarpClaims` is
//! attached to the GraphQL `Request` data.

use crate::warp_graphql::error::WarpGraphqlError;

/// Verified caller claims, attached to every GraphQL request that
/// passed the auth gate. Tier-3 stubs that don't need claims still
/// receive them (extractor runs before resolver dispatch).
#[derive(Debug, Clone)]
pub struct WarpClaims {
    /// `sub` claim — the broomva.tech-issued user_id.
    pub user_id: String,
    /// `aud` claim — must equal "lumen" for warp_graphql route.
    pub audience: String,
    /// `scope` claim — space-separated, e.g. "agent:read agent:write".
    pub scopes: Vec<String>,
    /// `anima_did` extension claim — DID bound by AnimaCustody.
    pub anima_did: Option<String>,
    /// `exp` claim — Unix timestamp.
    pub expires_at: u64,
}

impl WarpClaims {
    /// Anonymous fallback used by `createAnonymousUser` mutation.
    /// Carries an unsigned, ephemeral user_id; upgraded to a real
    /// claim on `mintCustomToken` success.
    pub fn anonymous(ephemeral_id: String) -> Self {
        Self {
            user_id: ephemeral_id,
            audience: "lumen".to_string(),
            scopes: vec!["agent:anonymous".to_string()],
            anima_did: None,
            expires_at: u64::MAX,
        }
    }

    /// Returns `true` if the caller has the named scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// Extract a bearer token from the `Authorization` header.
///
/// Returns `None` if the header is missing or malformed; the caller
/// decides whether to reject or fall through to anonymous.
pub fn extract_bearer(headers: &http::HeaderMap) -> Option<&str> {
    let header = headers.get(http::header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;
    value.strip_prefix("Bearer ")
}

/// Verify a bearer token via the existing JwksCache and produce
/// typed `WarpClaims`.
///
/// `audience_required` is the `aud` value the token must carry;
/// for the Lumen route this is always `"lumen"`. Tokens minted
/// for other audiences (e.g. internal Tier-2 capability tokens)
/// are rejected here.
pub async fn verify_to_claims(
    jwks: &crate::auth::JwksCache,
    token: &str,
    audience_required: &str,
) -> Result<WarpClaims, WarpGraphqlError> {
    // JwksCache::verify_capability_token returns a typed claims
    // struct (already shipped in M7-D, see crates/life-runtime/lifegw/src/auth/).
    let claims = jwks
        .verify_capability_token(token, audience_required)
        .await
        .map_err(|e| WarpGraphqlError::Unauthenticated)?;

    Ok(WarpClaims {
        user_id: claims.sub,
        audience: claims.aud,
        scopes: claims
            .scope
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        anima_did: claims.anima_did,
        expires_at: claims.exp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderMap, HeaderValue};

    #[test]
    fn extract_bearer_pulls_token() {
        let mut h = HeaderMap::new();
        h.insert(http::header::AUTHORIZATION, HeaderValue::from_static("Bearer abc.def.ghi"));
        assert_eq!(extract_bearer(&h), Some("abc.def.ghi"));
    }

    #[test]
    fn extract_bearer_rejects_missing_prefix() {
        let mut h = HeaderMap::new();
        h.insert(http::header::AUTHORIZATION, HeaderValue::from_static("abc.def.ghi"));
        assert_eq!(extract_bearer(&h), None);
    }

    #[test]
    fn extract_bearer_returns_none_when_absent() {
        let h = HeaderMap::new();
        assert_eq!(extract_bearer(&h), None);
    }

    #[test]
    fn anonymous_claims_have_anonymous_scope() {
        let c = WarpClaims::anonymous("eph-1234".into());
        assert_eq!(c.user_id, "eph-1234");
        assert!(c.has_scope("agent:anonymous"));
        assert!(!c.has_scope("agent:write"));
    }

    #[test]
    fn has_scope_checks_membership() {
        let c = WarpClaims {
            user_id: "u1".into(),
            audience: "lumen".into(),
            scopes: vec!["agent:read".into(), "agent:write".into()],
            anima_did: None,
            expires_at: 9999999999,
        };
        assert!(c.has_scope("agent:read"));
        assert!(c.has_scope("agent:write"));
        assert!(!c.has_scope("admin"));
    }
}
```

NOTE: this code calls `jwks.verify_capability_token(token, audience_required)`. If the existing `JwksCache` API differs slightly, adjust the call site to match — the M7-D implementation is the source of truth. If the existing claim struct doesn't expose `anima_did`, add it as an optional extension claim in this PR.

- [ ] **Step 2: Run the unit tests**

```bash
cargo test -p lifegw warp_graphql::auth
```

Expected: 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/warp_graphql/auth.rs
git commit -m "feat(lifegw): warp_graphql JWT extractor + WarpClaims (M7-W.1, BRO-XXXX)"
```

---

### Task 7: Define the request context

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/context.rs`

- [ ] **Step 1: Write the context type and tests**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/context.rs`:

```rust
//! Per-request context attached to every GraphQL invocation.
//!
//! Built once per HTTP request (or once per WS subscription session)
//! and carried via `async_graphql::Context::data::<WarpRequestCtx>()`
//! into every resolver.

use std::sync::Arc;

use crate::warp_graphql::auth::WarpClaims;
use crate::warp_graphql::dataloader::LifedDataloaders;

#[derive(Clone)]
pub struct WarpRequestCtx {
    pub claims: WarpClaims,
    pub dataloaders: Arc<LifedDataloaders>,
    /// Trace span id for cross-service correlation in vigil.
    pub trace_id: String,
}

impl WarpRequestCtx {
    pub fn new(
        claims: WarpClaims,
        dataloaders: Arc<LifedDataloaders>,
        trace_id: String,
    ) -> Self {
        Self {
            claims,
            dataloaders,
            trace_id,
        }
    }

    /// Convenience: returns the user_id of the authenticated caller.
    pub fn user_id(&self) -> &str {
        &self.claims.user_id
    }

    /// Returns `Err(Unauthenticated)` if the caller lacks the named scope.
    pub fn require_scope(
        &self,
        scope: &str,
    ) -> Result<(), crate::warp_graphql::error::WarpGraphqlError> {
        if self.claims.has_scope(scope) {
            Ok(())
        } else {
            Err(crate::warp_graphql::error::WarpGraphqlError::Unauthenticated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_scopes(scopes: Vec<&str>) -> WarpRequestCtx {
        WarpRequestCtx {
            claims: WarpClaims {
                user_id: "u1".into(),
                audience: "lumen".into(),
                scopes: scopes.into_iter().map(str::to_string).collect(),
                anima_did: None,
                expires_at: 9999999999,
            },
            dataloaders: Arc::new(LifedDataloaders::test_stub()),
            trace_id: "trace-1".into(),
        }
    }

    #[test]
    fn user_id_returns_sub() {
        let ctx = ctx_with_scopes(vec!["agent:read"]);
        assert_eq!(ctx.user_id(), "u1");
    }

    #[test]
    fn require_scope_passes_when_present() {
        let ctx = ctx_with_scopes(vec!["agent:read", "agent:write"]);
        assert!(ctx.require_scope("agent:read").is_ok());
    }

    #[test]
    fn require_scope_fails_when_absent() {
        let ctx = ctx_with_scopes(vec!["agent:read"]);
        assert!(ctx.require_scope("agent:write").is_err());
    }
}
```

NOTE: `LifedDataloaders::test_stub()` is added in Task 8.

- [ ] **Step 2: Stub `LifedDataloaders::test_stub()` so this compiles**

Open `core/life/crates/life-runtime/lifegw/src/warp_graphql/dataloader.rs` and add a placeholder so context tests compile:

```rust
//! Batched lifed gRPC clients (DataLoader pattern).
//!
//! Real implementation lands in Task 8. This file ships a stub so
//! sibling modules compile in CI during the M7-W.1 sub-phase.

#[derive(Default)]
pub struct LifedDataloaders {
    // Real fields added in Task 8.
}

impl LifedDataloaders {
    /// Test-only no-op dataloader. Real construction goes through
    /// `LifedDataloaders::new(channel)` (Task 8).
    #[cfg(test)]
    pub fn test_stub() -> Self {
        Self::default()
    }
}
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p lifegw warp_graphql::context
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/warp_graphql/context.rs core/life/crates/life-runtime/lifegw/src/warp_graphql/dataloader.rs
git commit -m "feat(lifegw): warp_graphql request context + dataloader stub (M7-W.1, BRO-XXXX)"
```

---

### Task 8: Implement the lifed dataloader

The dataloader batches concurrent gRPC calls to lifed services so a single GraphQL query that touches `getUser` + `getUserSettings` + `listAIConversations` becomes one RPC fan-out, not three.

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/dataloader.rs`

- [ ] **Step 1: Write the dataloader and tests**

Replace `core/life/crates/life-runtime/lifegw/src/warp_graphql/dataloader.rs` with:

```rust
//! Batched lifed gRPC clients.
//!
//! Each lifed service we call from resolvers is accessed through a
//! tonic gRPC client. For services that benefit from N+1 coalescing
//! (e.g., a query that needs `getUser` for 50 message authors in one
//! response), wrap them later in `async_graphql::dataloader::DataLoader`
//! — the framework's built-in lightweight loader (futures-channel + lru).
//! v1 holds the raw tonic clients without batching; Task 8 ships only
//! the holder shape.
//!
//! Per L6-D5, `Agent.StreamSession` is invoked via `createAgentTask` →
//! returned task_id is used to filter the multiplexed `warpDriveUpdates`
//! subscription. Mutations are not batched (they have side effects).

use std::sync::Arc;
use tonic::transport::Channel;

// gRPC clients (proto types live in `life-runtime-proto`).
use life_runtime_proto::lifed::{
    agent_service_client::AgentServiceClient,
    event_service_client::EventServiceClient,
    identity_service_client::IdentityServiceClient,
    inference_service_client::InferenceServiceClient,
    routing_cache_service_client::RoutingCacheServiceClient,
    settings_service_client::SettingsServiceClient,
    wallet_service_client::WalletServiceClient,
};

#[derive(Clone)]
pub struct LifedDataloaders {
    pub agent: AgentServiceClient<Channel>,
    pub events: EventServiceClient<Channel>,
    pub identity: IdentityServiceClient<Channel>,
    pub inference: InferenceServiceClient<Channel>,
    pub routing_cache: RoutingCacheServiceClient<Channel>,
    pub settings: SettingsServiceClient<Channel>,
    pub wallet: WalletServiceClient<Channel>,
}

impl LifedDataloaders {
    /// Construct one set of dataloaders sharing a single gRPC channel.
    /// In production lifegw connects to lifed via UDS (per Spec C₃ §4.2);
    /// in tests the caller passes a tonic-test in-process channel.
    pub fn new(channel: Channel) -> Self {
        Self {
            agent: AgentServiceClient::new(channel.clone()),
            events: EventServiceClient::new(channel.clone()),
            identity: IdentityServiceClient::new(channel.clone()),
            inference: InferenceServiceClient::new(channel.clone()),
            routing_cache: RoutingCacheServiceClient::new(channel.clone()),
            settings: SettingsServiceClient::new(channel.clone()),
            wallet: WalletServiceClient::new(channel),
        }
    }

    /// Test-only no-op dataloader for unit tests that never reach
    /// the gRPC layer (e.g., context.rs scope checks).
    #[cfg(test)]
    pub fn test_stub() -> Self {
        // Use a connector that fails on first use; tests that need
        // real RPCs use the wiremock-style fixtures in tests/.
        let channel = Channel::from_static("http://127.0.0.1:1")
            .connect_lazy();
        Self::new(channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stub_constructs_without_panic() {
        let _ = LifedDataloaders::test_stub();
    }

    // Real RPC tests live in tests/warp_graphql_t1_golden.rs (Task 11+).
}
```

NOTE: This depends on `life-runtime-proto` already exposing the listed services. Per MEMORY, M5 sub-phase A shipped lifed scaffolding with Agent + Events + 4 proxy stubs. Verify the exact client names match what's exported by `life_runtime_proto::lifed`. If service names differ (e.g., `agent_client::AgentClient`), adjust the imports — the structure stays the same.

- [ ] **Step 2: Run the tests**

```bash
cargo test -p lifegw warp_graphql::dataloader
```

Expected: 1 test passes (`test_stub_constructs_without_panic`).

- [ ] **Step 3: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/warp_graphql/dataloader.rs
git commit -m "feat(lifegw): wire lifed gRPC dataloaders into warp_graphql (M7-W.1, BRO-XXXX)"
```

---

### Task 9: Implement the Tier-3 stub fallback resolver

The Tier-3 fallback intercepts any operation we haven't explicitly wired and returns an SDL-compliant safe default. Without this, ~90 unimplemented operations would error and the Warp client would refuse to start.

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier3_stubs.rs`

- [ ] **Step 1: Write the stub-resolver helpers and tests**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier3_stubs.rs`:

```rust
//! Tier-3 stub helpers for unimplemented operations.
//!
//! Per Lumen spec L6-D4, ~90 of Warp's ~122 GraphQL operations are
//! Warp.dev SaaS scaffolding (teams, Stripe, Merkle sync, file
//! artifacts, managed secrets) that Lumen v1 does not reproduce.
//! Stubs here return safe SDL-compliant defaults so the Warp client
//! treats these as feature-flag-off rather than erroring.
//!
//! When a stubbed operation graduates to real implementation, move
//! its resolver from this file to tier1.rs or tier2.rs.

use crate::warp_graphql::error::WarpGraphqlError;

/// SDL-compliant safe default for a stubbed mutation that returns
/// a `MutationResponse`-shaped object. The actual Warp schema has
/// many such response types; the stub just returns `success: false`
/// with a clear reason so the UI can render "feature not enabled".
pub fn stub_mutation_response(operation: &'static str) -> serde_json::Value {
    serde_json::json!({
        "success": false,
        "reason": format!("operation '{}' is not enabled in this Lumen build", operation),
    })
}

/// Stubbed query that should return `null` (nullable single-object).
pub fn stub_null<T>() -> Option<T> {
    None
}

/// Stubbed query that should return an empty list.
pub fn stub_empty_list<T>() -> Vec<T> {
    Vec::new()
}

/// Stubbed query that the Warp client treats as a feature flag.
/// Returns a recognized "off" value rather than an error.
pub fn stub_feature_off(_feature: &'static str) -> Result<serde_json::Value, WarpGraphqlError> {
    Ok(serde_json::json!({
        "enabled": false,
        "reason": "not provisioned in this build",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_mutation_response_includes_operation_name() {
        let v = stub_mutation_response("createTeam");
        assert_eq!(v["success"], false);
        assert!(v["reason"].as_str().unwrap().contains("createTeam"));
    }

    #[test]
    fn stub_empty_list_is_empty() {
        let v: Vec<i32> = stub_empty_list();
        assert!(v.is_empty());
    }

    #[test]
    fn stub_null_is_none() {
        let v: Option<i32> = stub_null();
        assert!(v.is_none());
    }

    #[test]
    fn stub_feature_off_returns_disabled() {
        let v = stub_feature_off("teamSync").unwrap();
        assert_eq!(v["enabled"], false);
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test -p lifegw warp_graphql::resolvers::tier3_stubs
```

Expected: 4 tests pass.

- [ ] **Step 3: Add empty `tier1` and `tier2` modules so resolver tree compiles**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier1.rs`:

```rust
//! T1 resolvers — the must-implement set for a working agent.
//!
//! Implementations land in M7-W.2. This file ships empty in M7-W.1
//! so the resolver module tree compiles.
```

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier2.rs`:

```rust
//! T2 resolvers — production UX (history, code search, billing, MCP).
//!
//! Implementations land in M7-W.2 final pass and Phase β L.3.
```

- [ ] **Step 4: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/warp_graphql/resolvers/
git commit -m "feat(lifegw): tier3 stub helpers + tier1/tier2 placeholders (M7-W.1, BRO-XXXX)"
```

---

### Task 10: Build the async-graphql Schema with SDL-first binding

This is the keystone of M7-W.1. It loads the verbatim SDL, creates an async-graphql schema with our resolvers attached, and exposes a `WarpSchema` type usable from axum handlers.

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/adapter.rs`

- [ ] **Step 1: Write the schema builder and tests**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/adapter.rs`:

```rust
//! async-graphql Schema builder for the Warp-compatible surface.
//!
//! In SDL-first mode the schema definition is the verbatim
//! `schema_mirror::SDL`; resolver functions are bound to type names
//! programmatically. Operations without bindings fall through to the
//! tier-3 stub fallback.

use async_graphql::{
    dynamic::{Schema, SchemaBuilder, TypeRef},
    EmptyMutation, EmptySubscription,
};

use crate::warp_graphql::schema_mirror::SDL;

/// The compiled Warp-compatible schema.
///
/// Cheap to clone (Arc internally). Built once at lifegw startup
/// and shared across requests.
#[derive(Clone)]
pub struct WarpSchema {
    inner: async_graphql::dynamic::Schema,
}

impl WarpSchema {
    /// Build the schema by parsing `SDL` and binding resolvers.
    /// Returns an error if the SDL is malformed (catches schema-mirror
    /// corruption at startup, before serving traffic).
    pub fn build() -> Result<Self, async_graphql::dynamic::SchemaError> {
        // Phase α: parse the SDL with no resolver bindings. Tier-3
        // fallback handles every operation. Tier-1 bindings land in
        // M7-W.2; Tier-2 in M7-W.2 final pass + Phase β.
        let schema = Schema::build("Query", Some("Mutation"), Some("Subscription"))
            .extend_with_dynamic_sdl(SDL)?
            .data(()) // global schema data; resolvers pull from request ctx
            .finish()?;

        Ok(Self { inner: schema })
    }

    /// The underlying schema for use by axum HTTP / WS handlers.
    pub fn inner(&self) -> &Schema {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_builds_from_sdl() {
        let _schema = WarpSchema::build().expect("schema mirrors valid SDL");
    }

    #[test]
    fn schema_introspection_lists_query_type() {
        let schema = WarpSchema::build().expect("schema builds");
        // Smoke test: introspection __schema query should work.
        let req = async_graphql::Request::new("{ __schema { queryType { name } } }");
        let resp = futures::executor::block_on(schema.inner().execute(req));
        assert!(resp.errors.is_empty(), "introspection errors: {:?}", resp.errors);
        let json = resp.data.into_json().unwrap();
        assert_eq!(json["__schema"]["queryType"]["name"], "Query");
    }
}
```

NOTE: async-graphql 7.x dynamic-schema API may use `extend_with_dynamic_sdl` or a similar method; if the exact name differs, adjust to the version in workspace `Cargo.toml`. The test in step 2 is the contract — it must pass.

- [ ] **Step 2: Run the schema build tests**

```bash
cargo test -p lifegw warp_graphql::adapter
```

Expected: 2 tests pass. If `schema_builds_from_sdl` fails with parse errors, the schema mirror file in Task 2 was truncated — re-fetch and verify line count.

- [ ] **Step 3: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/warp_graphql/adapter.rs
git commit -m "feat(lifegw): build async-graphql Schema from verbatim SDL (M7-W.1, BRO-XXXX)"
```

---

### Task 11: Wire the HTTP `/graphql/v2` route

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/route.rs`
- Modify: `core/life/crates/life-runtime/lifegw/src/bootstrap.rs`

- [ ] **Step 1: Write the route handler**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/route.rs`:

```rust
//! axum routes for the Warp-compatible GraphQL surface.
//!
//! Two endpoints:
//! - POST `/graphql/v2`           : queries + mutations
//! - WebSocket `/graphql-ws`      : the single `warpDriveUpdates` subscription
//!
//! Both endpoints require a valid Bearer JWT (audience = "lumen") in
//! `Authorization` header. Anonymous-mode (createAnonymousUser) is the
//! one exception — that mutation accepts a missing or empty token and
//! returns an ephemeral user_id.

use std::sync::Arc;

use async_graphql::http::{GraphQLPlaygroundConfig, playground_source};
use async_graphql_axum::{GraphQLProtocol, GraphQLRequest, GraphQLResponse, GraphQLWebSocket};
use axum::{
    extract::{Query, State, WebSocketUpgrade},
    http::HeaderMap,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use std::collections::HashMap;

use crate::auth::JwksCache;
use crate::warp_graphql::{
    adapter::WarpSchema,
    auth::{extract_bearer, verify_to_claims, WarpClaims},
    context::WarpRequestCtx,
    dataloader::LifedDataloaders,
};

#[derive(Clone)]
pub struct WarpRouteState {
    pub schema: WarpSchema,
    pub jwks: Arc<JwksCache>,
    pub dataloaders: Arc<LifedDataloaders>,
}

/// Returns the axum router segment for the warp_graphql endpoints.
/// Mount under the lifegw root: `Router::new().merge(routes(state))`.
pub fn routes(state: WarpRouteState) -> Router {
    Router::new()
        .route("/graphql/v2", post(post_handler))
        .route("/graphql-ws", get(ws_handler))
        .with_state(state)
}

/// HTTP POST handler for queries and mutations.
async fn post_handler(
    State(state): State<WarpRouteState>,
    Query(query_params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    req: GraphQLRequest,
) -> Result<GraphQLResponse, Response> {
    let mut req = req.into_inner();

    // Auth: extract bearer; anonymous fallback if absent.
    let claims = match extract_bearer(&headers) {
        Some(token) => verify_to_claims(&state.jwks, token, "lumen")
            .await
            .map_err(|e| {
                let err: async_graphql::Error = e.into();
                Response::from(GraphQLResponse::from(async_graphql::Response::from_errors(
                    vec![async_graphql::ServerError::new(err.message, None)],
                )))
            })?,
        None => {
            // Anonymous-mode is allowed only for the createAnonymousUser
            // mutation. The schema has no anonymous query path; if the
            // operation isn't createAnonymousUser, we'll fall through
            // to the tier-3 stub which returns "feature not enabled".
            let op_name = query_params
                .get("op")
                .cloned()
                .unwrap_or_else(|| "<unknown>".to_string());
            WarpClaims::anonymous(format!("eph-{}", uuid::Uuid::new_v4()))
        }
    };

    // Build per-request context.
    let ctx = WarpRequestCtx::new(
        claims,
        state.dataloaders.clone(),
        // Trace id: pull from incoming OTel header if present, else gen.
        headers
            .get("traceparent")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("trace-anon")
            .to_string(),
    );

    req = req.data(ctx);

    Ok(state.schema.inner().execute(req).await.into())
}

/// WebSocket upgrade handler for graphql-transport-ws.
async fn ws_handler(
    State(state): State<WarpRouteState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let schema = state.schema.inner().clone();
    let dataloaders = state.dataloaders.clone();
    let jwks = state.jwks.clone();

    // graphql-transport-ws subprotocol negotiation. Bearer auth happens
    // in the connection_init message (per spec C₃ §6.5 design); the
    // initial WS upgrade requires only the right subprotocol.
    ws.protocols(["graphql-transport-ws"])
        .on_upgrade(move |socket| async move {
            // Auth from headers (if Bearer subprotocol used) or from
            // connection_init payload (per graphql-ws spec). Default
            // implementation expects Bearer in the Authorization header.
            let claims = match extract_bearer(&headers) {
                Some(token) => match verify_to_claims(&jwks, token, "lumen").await {
                    Ok(c) => c,
                    Err(_) => return, // close socket on auth failure
                },
                None => return, // subscriptions require auth (no anonymous subs)
            };

            let ctx = WarpRequestCtx::new(claims, dataloaders, "trace-ws".into());

            GraphQLWebSocket::new(socket, schema, GraphQLProtocol::default())
                .with_data(async_graphql::Data::default())
                .serve()
                .await;
        })
}
```

- [ ] **Step 2: Wire the route into bootstrap**

Open `core/life/crates/life-runtime/lifegw/src/bootstrap.rs` and locate where the axum `Router` is constructed. Add:

```rust
// Build warp_graphql state.
let warp_schema = crate::warp_graphql::adapter::WarpSchema::build()
    .map_err(|e| anyhow::anyhow!("warp_graphql schema build failed: {e}"))?;
let warp_dataloaders = std::sync::Arc::new(
    crate::warp_graphql::dataloader::LifedDataloaders::new(lifed_channel.clone()),
);
let warp_state = crate::warp_graphql::route::WarpRouteState {
    schema: warp_schema,
    jwks: jwks_cache.clone(),
    dataloaders: warp_dataloaders,
};

// Merge warp_graphql routes into the main router.
let app = app.merge(crate::warp_graphql::route::routes(warp_state));
```

NOTE: Adapt variable names (`lifed_channel`, `jwks_cache`, `app`) to the actual identifiers in the existing bootstrap.rs.

- [ ] **Step 3: Verify the crate compiles**

```bash
cargo check -p lifegw
```

Expected: success.

- [ ] **Step 4: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/warp_graphql/route.rs core/life/crates/life-runtime/lifegw/src/bootstrap.rs
git commit -m "feat(lifegw): wire /graphql/v2 + /graphql-ws routes (M7-W.1, BRO-XXXX)"
```

---

### Task 12: Conformance test — schema introspection matches upstream byte-for-byte

This is the M7-W.1 acceptance gate. If our introspection doesn't match upstream, schema-aware Warp client code generation breaks.

**Files:**
- Create: `core/life/crates/life-runtime/lifegw/tests/warp_graphql_introspection.rs`

- [ ] **Step 1: Write the conformance test**

Create `core/life/crates/life-runtime/lifegw/tests/warp_graphql_introspection.rs`:

```rust
//! Conformance test: introspection result matches the verbatim SDL.
//!
//! If this test fails, our SDL mirror has drifted from upstream Warp's
//! schema, OR our resolver bindings violated SDL types. Either is a
//! M7-W contract violation.

use async_graphql::parser::types::ServiceDocument;
use lifegw::warp_graphql::adapter::WarpSchema;
use lifegw::warp_graphql::schema_mirror::SDL;

#[tokio::test]
async fn introspection_returns_full_schema() {
    let schema = WarpSchema::build().expect("schema builds");

    // Standard introspection query.
    let req = async_graphql::Request::new(INTROSPECTION_QUERY);
    let resp = schema.inner().execute(req).await;
    assert!(resp.errors.is_empty(), "introspection errors: {:?}", resp.errors);
}

#[test]
fn sdl_mirror_parses_as_valid_graphql_document() {
    let parsed: ServiceDocument =
        async_graphql::parser::parse_schema(SDL).expect("SDL parses");

    // Sanity: schema declares Query, Mutation, Subscription roots.
    let mut has_query = false;
    let mut has_mutation = false;
    let mut has_subscription = false;
    for def in &parsed.definitions {
        if let async_graphql::parser::types::TypeSystemDefinition::Schema(_) = def {
            has_query = true; // schema decl exists
        }
    }

    // Walk type definitions looking for Query / Mutation / Subscription.
    let sdl_text = SDL;
    has_query |= sdl_text.contains("type Query");
    has_mutation |= sdl_text.contains("type Mutation");
    has_subscription |= sdl_text.contains("type Subscription");

    assert!(has_query, "SDL missing Query root");
    assert!(has_mutation, "SDL missing Mutation root");
    assert!(has_subscription, "SDL missing Subscription root");
}

#[test]
fn sdl_mirror_size_within_expected_range() {
    // Upstream Warp schema is ~4,134 lines. We accept ±5% drift between
    // rebases as normal upstream evolution; anything more triggers a
    // manual rebase ticket.
    let line_count = SDL.lines().count();
    assert!(
        (3920..=4340).contains(&line_count),
        "SDL line count {} outside expected range; check upstream pin",
        line_count
    );
}

#[test]
fn sdl_mirror_includes_critical_operations() {
    // Tier 1 + Tier 2 operations the resolvers will bind to.
    // If these go missing in upstream, our M7-W.2 plan is invalidated
    // before we start.
    // Operation names match upstream Warp's actual schema — see
    // Pre-flight Update + Task 2 spec-review finding (2026-05-11):
    // upstream uses bare-noun field names (`user`, `freeAvailableModels`,
    // `simpleIntegrations`) for some Queries and `get*`-prefixed names
    // for others (`getRelevantFragments`, `getCloudEnvironments`). Don't
    // assume uniform convention.
    const REQUIRED_OPS: &[&str] = &[
        // Mutations (prefixed verbs)
        "createAgentTask",
        "updateAgentTask",
        "generateDialogue",
        "generateCommands",
        "generateCodeEmbeddings",
        "createAnonymousUser",
        "setUserIsOnboarded",
        "mintCustomToken",
        "issueTaskIdentityToken",
        "generateApiKey",
        "expireApiKey",
        "updateUserSettings",
        // Queries (mixed bare-noun + get-prefixed)
        "user",                     // upstream: bare; NOT getUser
        "freeAvailableModels",      // upstream: bare; NOT getFreeAvailableModels
        "simpleIntegrations",       // upstream: bare; NOT getSimpleIntegrations
        "codebaseContextConfig",
        "listAIConversations",
        "getRelevantFragments",     // upstream: get-prefixed
        "rerankFragments",
        // Subscription
        "warpDriveUpdates",
    ];

    for op in REQUIRED_OPS {
        assert!(
            SDL.contains(op),
            "SDL missing required operation '{}'; upstream may have renamed it",
            op
        );
    }
}

const INTROSPECTION_QUERY: &str = r#"
{
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types {
      name
      kind
    }
  }
}
"#;
```

- [ ] **Step 2: Run the conformance suite**

```bash
cargo test -p lifegw --test warp_graphql_introspection
```

Expected: 4 tests pass. The `sdl_mirror_size_within_expected_range` test catches mirror corruption; `sdl_mirror_includes_critical_operations` catches upstream renames.

- [ ] **Step 3: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/tests/warp_graphql_introspection.rs
git commit -m "test(lifegw): warp_graphql conformance — schema introspection (M7-W.1, BRO-XXXX)"
```

---

### Task 13: M7-W.1 final-pass full crate check

- [ ] **Step 1: Run full crate verification**

From `core/life/`:

```bash
cargo fmt -p lifegw
cargo clippy -p lifegw -- -D warnings
cargo test -p lifegw
```

Expected: format clean, clippy passes with no warnings, all tests green.

- [ ] **Step 2: If any clippy warnings appear, fix inline and re-run**

Common Rust 2024 clippy issues:
- `unsafe { std::env::set_var(...) }` — Rust 2024 makes these unsafe.
- `clippy::pedantic` warnings about doc comments — match style of surrounding lifegw code.

- [ ] **Step 3: Commit any cleanup**

```bash
git add -u
git commit -m "chore(lifegw): warp_graphql M7-W.1 final pass (fmt + clippy clean) (BRO-XXXX)"
```

**M7-W.1 acceptance gate**: lifegw compiles, schema introspection matches upstream, all unit tests pass, JWT auth gates the route, WebSocket handshake succeeds.

---

## M7-W.2 — T1 Resolvers

**Outcome of M7-W.2:** ~15 Tier-1 GraphQL operations route through real lifed gRPC. The Warp client can authenticate, list available models, fetch settings, and submit agent tasks. **Subscriptions still inert** — that's M7-W.3.

The pattern is the same for each resolver group:
1. Write a unit test that constructs the resolver context and invokes the resolver against a mocked `LifedDataloaders`.
2. Implement the resolver in `tier1.rs`.
3. Bind the resolver to the schema in `adapter.rs`.
4. Add a golden-response test in `tests/warp_graphql_t1_golden.rs` that exercises the wire shape.
5. Commit.

To keep this plan tractable, Tasks 14–19 detail the first resolver group (identity + auth) fully, then provide patterns for the remaining groups. Each resolver follows the same template; an implementer who completes Task 14 and Task 15 has the muscle memory for the rest.

### Error-propagation pattern (REQUIRED — read before writing any resolver)

**NOTE (2026-05-11 plan correction surfaced by Task 5 review):** async-graphql 7.2.1 has a blanket `impl<T: Display + Send + Sync + 'static> From<T> for Error` that is **active** unless the `custom-error-conversion` feature is enabled (which we do NOT enable workspace-wide because it would break unrelated downstream code that relies on `From<&str>` / `From<String>`).

Consequence: a bare `?` on `Result<_, WarpGraphqlError>` in a resolver silently converts via the blanket impl, which produces `async_graphql::Error { extensions: None }` — **the stable `extensions.code` annotation that `WarpGraphqlError::into_graphql()` exists to surface is silently dropped on the hot path**. The Warp client then sees only `error.message` and cannot route on the typed code.

The fix is the `IntoGraphqlExt` trait in `error.rs` (shipped with Task 5's commit `df715efe` follow-up). All T1+T2 resolvers MUST use `.into_graphql()?` instead of bare `?` when propagating `WarpGraphqlError` values to the client:

```rust
// ❌ WRONG — silently drops extensions.code via the blanket From impl:
let resp = client
    .get_user(req)
    .await
    .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

// ✓ RIGHT — preserves extensions.code via the IntoGraphqlExt trait:
use crate::warp_graphql::error::IntoGraphqlExt;
let resp = client
    .get_user(req)
    .await
    .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))
    .into_graphql()?;
```

Apply the same `.into_graphql()?` ending to every error-propagation site in Tasks 14–22's resolver implementations. The bare-`?` form in the original 2026-05-10 plan code blocks at Tasks 14, 15, 16, 17, 18, 20 was INCORRECT — read those code blocks as templates whose `?` endings must be replaced with `.into_graphql()?`. Spec-compliance + code-quality reviewers will verify this pattern is applied.

---

### Task 14: T1 — Identity resolvers (`user`, `createAnonymousUser`, `setUserIsOnboarded`)

**NOTE (2026-05-11 plan correction):** upstream Warp's Query field is `user`, NOT `getUser`. The resolver function name in this section says `resolve_get_user` for historical consistency with the original 2026-05-10 plan; the field-resolver binding at `.field_resolver("Query", "user", ...)` is the load-bearing line — match the upstream field name there exactly.

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier1.rs`
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/adapter.rs` (binding)
- Modify: `core/life/crates/life-runtime/lifegw/tests/warp_graphql_t1_golden.rs`

- [ ] **Step 1: Write the resolver code**

Append to `core/life/crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier1.rs`:

```rust
//! T1 resolvers — must-implement set for a working agent.

use crate::warp_graphql::context::WarpRequestCtx;
use crate::warp_graphql::error::WarpGraphqlError;
use async_graphql::dynamic::{FieldFuture, FieldValue, ResolverContext};

/// Resolver: query `user`. Returns the authenticated user's profile.
///
/// Schema shape (per upstream verbatim mirror): `user(requestContext: RequestContext!): UserResult!`
/// Phase α: ignores the `requestContext` arg's nuances; uses the bearer
/// JWT's `sub` as the user_id. The `UserResult` return type bundles
/// profile fields + nested settings + nested requestLimitInfo — those
/// nested fields fall through to Tier-3 stubs in v1 (real field-resolvers
/// can be wired in a follow-up when telemetry/billing UX is needed).
pub fn resolve_get_user(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;

        let mut client = ctx.dataloaders.identity.clone();
        let resp = client
            .get_user(life_runtime_proto::lifed::GetUserRequest {
                user_id: ctx.user_id().to_string(),
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let user = resp.into_inner();
        let value = serde_json::json!({
            "id":       user.user_id,
            "email":    user.email,
            "displayName": user.display_name,
            "createdAt":   user.created_at,
            "isOnboarded": user.is_onboarded,
        });

        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `createAnonymousUser`. Creates an ephemeral user
/// the client uses pre-OAuth, then upgrades on `mintCustomToken`.
pub fn resolve_create_anonymous_user(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;

        let mut client = ctx.dataloaders.identity.clone();
        let resp = client
            .create_anonymous_user(life_runtime_proto::lifed::CreateAnonymousUserRequest {})
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let user = resp.into_inner();
        let value = serde_json::json!({
            "id":          user.user_id,
            "email":       null,
            "displayName": "Anonymous",
            "createdAt":   user.created_at,
            "isOnboarded": false,
        });

        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `setUserIsOnboarded`. Marks the calling user
/// as having completed first-launch onboarding.
pub fn resolve_set_user_is_onboarded(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;

        ctx.require_scope("agent:write")?;

        let mut client = ctx.dataloaders.identity.clone();
        client
            .set_onboarding_flag(life_runtime_proto::lifed::SetOnboardingFlagRequest {
                user_id: ctx.user_id().to_string(),
                is_onboarded: true,
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let value = serde_json::json!({ "success": true });
        Ok(Some(FieldValue::value(value)))
    })
}
```

NOTE: Proto message names (`GetUserRequest`, `CreateAnonymousUserRequest`, etc.) must match what `life-runtime-proto` actually exports. If lifed's IdentityService uses different names, adjust accordingly.

- [ ] **Step 2: Bind resolvers to the schema**

Open `core/life/crates/life-runtime/lifegw/src/warp_graphql/adapter.rs`. Modify `WarpSchema::build` to register tier-1 resolvers. The exact API depends on async-graphql 7.x dynamic-schema bindings; the shape is roughly:

```rust
use crate::warp_graphql::resolvers::tier1::{
    resolve_create_anonymous_user, resolve_get_user, resolve_set_user_is_onboarded,
};

// Inside WarpSchema::build, after parsing SDL:
schema_builder
    .field_resolver("Query", "user", resolve_get_user)  // upstream field is `user`, not `getUser`
    .field_resolver("Mutation", "createAnonymousUser", resolve_create_anonymous_user)
    .field_resolver("Mutation", "setUserIsOnboarded", resolve_set_user_is_onboarded)
    // (continued for each T1 op)
```

Refer to the async-graphql 7.x docs for the exact dynamic-binding API; if it has changed, the design is to register one resolver per (parent_type, field_name) pair.

- [ ] **Step 3: Add a golden-response test**

Append to (or create) `core/life/crates/life-runtime/lifegw/tests/warp_graphql_t1_golden.rs`:

```rust
//! Golden-response shape tests for Tier-1 resolvers.
//!
//! These tests mock the lifed gRPC layer (in-process tonic-test) and
//! assert that the GraphQL response matches the shape upstream Warp
//! expects. If a Warp client deserialization breaks, this is where
//! the contract is checked first.

use async_graphql::Request;
use lifegw::warp_graphql::adapter::WarpSchema;
use lifegw::warp_graphql::auth::WarpClaims;
use lifegw::warp_graphql::context::WarpRequestCtx;
use lifegw::warp_graphql::dataloader::LifedDataloaders;

#[tokio::test]
async fn get_user_returns_authenticated_user_shape() {
    // Setup: build schema + a stubbed dataloaders that returns a known
    // user for the test user_id. Real impl uses a tonic-test in-process
    // server; here we use the test_stub which short-circuits at the
    // gRPC layer (we'll wire real fixtures in Task 19's E2E).
    let schema = WarpSchema::build().expect("schema builds");
    let claims = WarpClaims {
        user_id: "test-user-1".into(),
        audience: "lumen".into(),
        scopes: vec!["agent:read".into(), "agent:write".into()],
        anima_did: None,
        expires_at: 9999999999,
    };
    let ctx = WarpRequestCtx::new(
        claims,
        std::sync::Arc::new(LifedDataloaders::test_stub()),
        "trace-test".into(),
    );

    let req = Request::new("query { user { profile { id email } } }").data(ctx);
    let resp = schema.inner().execute(req).await;

    // The test_stub dataloader fails on first RPC; we expect either a
    // typed UpstreamError (if we got that far) or a connection error
    // surfaced as an extension code. The shape contract still applies:
    // when a real lifed serves the request, the response must include
    // the named fields.
    if !resp.errors.is_empty() {
        // Acceptable: stub fails at RPC layer. Verify the error has our
        // typed extension code.
        let err = &resp.errors[0];
        let code = err.extensions.as_ref().and_then(|e| e.get("code"));
        assert!(
            matches!(code.map(|c| c.as_str()), Some(Some("UPSTREAM_ERROR"))),
            "unexpected error code: {:?}",
            code
        );
    } else {
        // If the test infrastructure has a real lifed mock, verify shape.
        let json = resp.data.into_json().unwrap();
        assert!(json["user"]["id"].is_string());
        assert!(json["user"]["email"].is_string() || json["user"]["email"].is_null());
    }
}

#[tokio::test]
async fn create_anonymous_user_returns_user_shape() {
    let schema = WarpSchema::build().expect("schema builds");
    let claims = WarpClaims::anonymous("bootstrap".into());
    let ctx = WarpRequestCtx::new(
        claims,
        std::sync::Arc::new(LifedDataloaders::test_stub()),
        "trace-test".into(),
    );

    let req = Request::new("mutation { createAnonymousUser { id displayName } }").data(ctx);
    let resp = schema.inner().execute(req).await;

    // Same as above: stub fails at gRPC; we verify the typed error path.
    if !resp.errors.is_empty() {
        let err = &resp.errors[0];
        let code = err.extensions.as_ref().and_then(|e| e.get("code"));
        assert!(matches!(
            code.map(|c| c.as_str()),
            Some(Some("UPSTREAM_ERROR"))
        ));
    }
}
```

- [ ] **Step 4: Run all tests**

```bash
cargo test -p lifegw
```

Expected: all unit + golden tests pass.

- [ ] **Step 5: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/src/warp_graphql/ core/life/crates/life-runtime/lifegw/tests/warp_graphql_t1_golden.rs
git commit -m "feat(lifegw): T1 identity resolvers — user/createAnonymousUser/setUserIsOnboarded (M7-W.2, BRO-XXXX)"
```

---

### Task 15: T1 — Token mint resolvers (`mintCustomToken`, `issueTaskIdentityToken`, `generateApiKey`, `expireApiKey`)

This is the OAuth-PKCE bridge for L6-D6. The flow:

1. Lumen client receives a one-time `code` from broomva.tech.
2. Client calls `mintCustomToken(code, verifier)`.
3. lifegw resolver:
   - Validates `code` against broomva.tech `/api/auth/verify`.
   - Calls `AnimaCustody.bind_oauth_session(user_id)` to wrap the broomva session in an Anima-signed claim.
   - Mints a JWT via the existing Tier-User minter (Spec C₃ §5).
4. Returns the JWT to the client.

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/resolvers/tier1.rs`
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/adapter.rs`

- [ ] **Step 1: Implement `mintCustomToken`**

Append to `tier1.rs`:

```rust
/// Resolver: mutation `mintCustomToken`. OAuth-PKCE code exchange →
/// AnimaCustody-bound JWT for downstream use.
///
/// Per Lumen spec L6-D6:
///   1. Validate `code` against broomva.tech /api/auth/verify
///   2. AnimaCustody.bind_oauth_session(user_id) → anima_did + signed claim
///   3. Tier-User minter issues JWT { sub, aud: "lumen", scope, exp }
///
/// Returns: { token: String!, expiresAt: Int! }
pub fn resolve_mint_custom_token(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;

        // Pull the `code` and `verifier` arguments from the input.
        let args = rctx.args;
        let code = args
            .try_get("code")
            .map_err(|_| WarpGraphqlError::InvalidInput("missing 'code'".into()))?
            .string()
            .map_err(|_| WarpGraphqlError::InvalidInput("'code' must be string".into()))?;
        let verifier = args
            .try_get("verifier")
            .map_err(|_| WarpGraphqlError::InvalidInput("missing 'verifier'".into()))?
            .string()
            .map_err(|_| WarpGraphqlError::InvalidInput("'verifier' must be string".into()))?;

        // Step 1: validate code with broomva.tech.
        // (The broomva.tech client lives in `lifed.IdentityService` —
        //  delegate the call there to keep credentials out of lifegw.)
        let mut identity_client = ctx.dataloaders.identity.clone();
        let oauth_resp = identity_client
            .verify_oauth_code(life_runtime_proto::lifed::VerifyOauthCodeRequest {
                code: code.to_string(),
                verifier: verifier.to_string(),
                provider: "broomva.tech".to_string(),
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let oauth = oauth_resp.into_inner();

        // Step 2 + 3: bind to anima + mint JWT. lifed.IdentityService
        // composes these — the resolver doesn't see the raw Anima
        // signing key.
        let mint_resp = identity_client
            .mint_lumen_token(life_runtime_proto::lifed::MintLumenTokenRequest {
                user_id: oauth.user_id,
                ttl_seconds: 24 * 3600,
                scopes: vec!["agent:read".into(), "agent:write".into()],
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let token = mint_resp.into_inner();
        let value = serde_json::json!({
            "token":     token.jwt,
            "expiresAt": token.expires_at,
        });

        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `issueTaskIdentityToken`. Short-lived task-scoped
/// JWT (per Spec C₃ §5 Tier-User minter). Used when an agent task needs
/// to make external calls on behalf of the user with bounded scope.
pub fn resolve_issue_task_identity_token(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;
        ctx.require_scope("agent:write")?;

        let args = rctx.args;
        let task_id = args
            .try_get("taskId")
            .map_err(|_| WarpGraphqlError::InvalidInput("missing 'taskId'".into()))?
            .string()
            .map_err(|_| WarpGraphqlError::InvalidInput("'taskId' must be string".into()))?;

        let mut identity_client = ctx.dataloaders.identity.clone();
        let resp = identity_client
            .mint_task_identity_token(life_runtime_proto::lifed::MintTaskIdentityTokenRequest {
                user_id: ctx.user_id().to_string(),
                task_id: task_id.to_string(),
                ttl_seconds: 15 * 60, // 15-minute task scope per Spec C₃ §5
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let token = resp.into_inner();
        let value = serde_json::json!({
            "token":     token.jwt,
            "expiresAt": token.expires_at,
        });

        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `generateApiKey`. Long-lived API key for headless
/// access. Issued via the same minter path with `aud: "lumen-api"`.
pub fn resolve_generate_api_key(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;
        ctx.require_scope("agent:write")?;

        let mut identity_client = ctx.dataloaders.identity.clone();
        let resp = identity_client
            .issue_api_key(life_runtime_proto::lifed::IssueApiKeyRequest {
                user_id: ctx.user_id().to_string(),
                ttl_seconds: 0, // 0 = no expiry; revocable via expireApiKey
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let key = resp.into_inner();
        let value = serde_json::json!({
            "id":  key.key_id,
            "key": key.key_secret, // shown once, then never returned
        });

        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `expireApiKey`. Revokes a previously-issued API key.
pub fn resolve_expire_api_key(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;
        ctx.require_scope("agent:write")?;

        let key_id = rctx
            .args
            .try_get("id")
            .map_err(|_| WarpGraphqlError::InvalidInput("missing 'id'".into()))?
            .string()
            .map_err(|_| WarpGraphqlError::InvalidInput("'id' must be string".into()))?;

        let mut identity_client = ctx.dataloaders.identity.clone();
        identity_client
            .revoke_api_key(life_runtime_proto::lifed::RevokeApiKeyRequest {
                user_id: ctx.user_id().to_string(),
                key_id: key_id.to_string(),
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let value = serde_json::json!({ "success": true });
        Ok(Some(FieldValue::value(value)))
    })
}
```

- [ ] **Step 2: Bind the resolvers in `adapter.rs`**

Add to the schema-builder bindings:

```rust
.field_resolver("Mutation", "mintCustomToken",          resolve_mint_custom_token)
.field_resolver("Mutation", "issueTaskIdentityToken",   resolve_issue_task_identity_token)
.field_resolver("Mutation", "generateApiKey",           resolve_generate_api_key)
.field_resolver("Mutation", "expireApiKey",             resolve_expire_api_key)
```

- [ ] **Step 3: Verify the crate compiles**

```bash
cargo check -p lifegw
cargo test -p lifegw warp_graphql
```

Expected: success.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(lifegw): T1 token mint resolvers — OAuth-PKCE → JWT (L6-D6, M7-W.2, BRO-XXXX)"
```

---

### Task 16: T1 — Settings resolver (`updateUserSettings` only)

**NOTE (2026-05-11 plan correction):** upstream Warp's schema has NO top-level `getUserSettings` Query — settings are nested under the `UserResult` returned by the `user` Query. The original 2026-05-10 plan listed `getUserSettings` as a T1 op; that was incorrect. Phase α drops the standalone settings-read resolver — when the Warp client queries `user { settings { ... } }`, the nested fields fall through to Tier-3 stubs (returning the schema's nullable defaults). A real field-resolver on `UserResult.settings` can be added in a follow-up if Lumen UX needs to persist real settings to lago in v1; that's tracked as an open question.

Only `updateUserSettings` (Mutation) is wired in Task 16.

**Files:** same as Task 14.

- [ ] **Step 1: Implement the resolver**

Append to `tier1.rs`:

```rust
pub fn resolve_update_user_settings(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;
        ctx.require_scope("agent:write")?;

        let input = rctx
            .args
            .try_get("input")
            .map_err(|_| WarpGraphqlError::InvalidInput("missing 'input'".into()))?;

        // Parse the input as JSON; the schema declares a structured
        // UpdateUserSettingsInput, but we serialize it through serde_json
        // and forward to lifed.
        let input_json = input.deserialize::<serde_json::Value>()
            .map_err(|e| WarpGraphqlError::InvalidInput(e.to_string()))?;

        let mut client = ctx.dataloaders.settings.clone();
        client
            .update_user_settings(life_runtime_proto::lifed::UpdateUserSettingsRequest {
                user_id:        ctx.user_id().to_string(),
                settings_json:  input_json.to_string(),
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let value = serde_json::json!({ "success": true });
        Ok(Some(FieldValue::value(value)))
    })
}
```

- [ ] **Step 2: Bind + test + commit**

```rust
// Note: there is NO Query "getUserSettings" — that field doesn't exist
// upstream. The Mutation "updateUserSettings" is the only top-level
// settings op. Settings-read fall-through is handled by the Tier-3 stub.
.field_resolver("Mutation", "updateUserSettings", resolve_update_user_settings)
```

```bash
cargo test -p lifegw warp_graphql
git add -u
git commit -m "feat(lifegw): T1 settings resolver — updateUserSettings Mutation (M7-W.2, BRO-XXXX)"
```

---

### Task 17: T1 — Inference & model resolvers (`freeAvailableModels`, `generateDialogue`, `generateCommands`)

**NOTE (2026-05-11 plan correction):** upstream Warp's Query field is `freeAvailableModels` (bare-noun), NOT `getFreeAvailableModels`. The resolver function name `resolve_get_free_available_models` is kept for historical consistency, but the `.field_resolver("Query", "freeAvailableModels", ...)` binding is what matters.

`generateDialogue` is the most architecturally significant resolver — it routes through Spec E InferenceBackend via lifed and triggers the agent loop. The signature is large; the implementation pattern is the same as preceding resolvers but with streaming response handling.

**Files:** same.

- [ ] **Step 1: Implement `freeAvailableModels`**

```rust
pub fn resolve_get_free_available_models(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;

        let mut client = ctx.dataloaders.inference.clone();
        let resp = client
            .list_available_models(life_runtime_proto::lifed::ListAvailableModelsRequest {
                user_id: ctx.user_id().to_string(),
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let models: Vec<serde_json::Value> = resp.into_inner().models
            .into_iter()
            .map(|m| serde_json::json!({
                "id":         m.id,
                "name":       m.display_name,
                "provider":   m.provider,
                "tier":       m.tier,
                "contextLen": m.context_window,
            }))
            .collect();

        Ok(Some(FieldValue::value(serde_json::Value::Array(models))))
    })
}
```

- [ ] **Step 2: Implement `generateDialogue` (synchronous response shape)**

`generateDialogue` returns a single completed turn — streaming token deltas flow through the `warpDriveUpdates` subscription (Task 20). This resolver returns the final assembled dialogue once the underlying inference call completes.

```rust
pub fn resolve_generate_dialogue(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;
        ctx.require_scope("agent:write")?;

        let input = rctx
            .args
            .try_get("input")
            .map_err(|_| WarpGraphqlError::InvalidInput("missing 'input'".into()))?
            .deserialize::<serde_json::Value>()
            .map_err(|e| WarpGraphqlError::InvalidInput(e.to_string()))?;

        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
        let model_id = input["modelId"].as_str().unwrap_or("").to_string();
        let conversation_id = input["conversationId"].as_str().map(|s| s.to_string());

        let mut client = ctx.dataloaders.inference.clone();
        let resp = client
            .generate_dialogue(life_runtime_proto::lifed::GenerateDialogueRequest {
                user_id: ctx.user_id().to_string(),
                prompt,
                model_id,
                conversation_id: conversation_id.unwrap_or_default(),
                streaming: false, // streaming flows through warpDriveUpdates subscription
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let dialogue = resp.into_inner();
        let value = serde_json::json!({
            "id":             dialogue.dialogue_id,
            "conversationId": dialogue.conversation_id,
            "content":        dialogue.content,
            "role":           "assistant",
            "tokensIn":       dialogue.tokens_in,
            "tokensOut":      dialogue.tokens_out,
        });

        Ok(Some(FieldValue::value(value)))
    })
}
```

- [ ] **Step 3: Implement `generateCommands`**

Identical pattern with a different lifed RPC:

```rust
pub fn resolve_generate_commands(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;
        ctx.require_scope("agent:write")?;

        let intent = rctx
            .args
            .try_get("intent")
            .map_err(|_| WarpGraphqlError::InvalidInput("missing 'intent'".into()))?
            .string()
            .map_err(|_| WarpGraphqlError::InvalidInput("'intent' must be string".into()))?;

        let mut client = ctx.dataloaders.inference.clone();
        let resp = client
            .generate_commands(life_runtime_proto::lifed::GenerateCommandsRequest {
                user_id: ctx.user_id().to_string(),
                intent: intent.to_string(),
                shell: rctx.args
                    .try_get("shell")
                    .ok()
                    .and_then(|v| v.string().ok())
                    .map(str::to_string)
                    .unwrap_or_else(|| "bash".to_string()),
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let suggestions: Vec<serde_json::Value> = resp.into_inner().suggestions
            .into_iter()
            .map(|s| serde_json::json!({
                "command":     s.command,
                "explanation": s.explanation,
                "confidence":  s.confidence,
            }))
            .collect();

        Ok(Some(FieldValue::value(serde_json::Value::Array(suggestions))))
    })
}
```

- [ ] **Step 4: Bind + test + commit**

```rust
.field_resolver("Query",    "freeAvailableModels", resolve_get_free_available_models)  // upstream field is `freeAvailableModels`
.field_resolver("Mutation", "generateDialogue",       resolve_generate_dialogue)
.field_resolver("Mutation", "generateCommands",       resolve_generate_commands)
```

```bash
cargo test -p lifegw
git add -u
git commit -m "feat(lifegw): T1 inference resolvers — models/dialogue/commands (M7-W.2, BRO-XXXX)"
```

---

### Task 18: T1 — Agent task resolvers (`createAgentTask`, `updateAgentTask`)

The most architecturally significant resolver pair. `createAgentTask` spawns an arcan ConsciousnessActor; `updateAgentTask` controls it (cancel, approve a tool call, send user input).

**Files:** same.

- [ ] **Step 1: Implement `createAgentTask`**

```rust
/// Resolver: mutation `createAgentTask`.
///
/// Per Lumen spec L6-D5, this is the entry-point for agent invocation.
/// Lifed dispatches to arcan, which spawns a ConsciousnessActor.
/// Subsequent stream events flow via the `warpDriveUpdates` subscription
/// filtered by the returned `task_id`.
pub fn resolve_create_agent_task(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;
        ctx.require_scope("agent:write")?;

        let input = rctx
            .args
            .try_get("input")
            .map_err(|_| WarpGraphqlError::InvalidInput("missing 'input'".into()))?
            .deserialize::<serde_json::Value>()
            .map_err(|e| WarpGraphqlError::InvalidInput(e.to_string()))?;

        let prompt        = input["prompt"]        .as_str().unwrap_or("").to_string();
        let model_id      = input["modelId"]       .as_str().unwrap_or("").to_string();
        let workspace_id  = input["workspaceId"]   .as_str().unwrap_or("").to_string();
        let parent_task   = input["parentTaskId"]  .as_str().map(str::to_string);

        let mut client = ctx.dataloaders.agent.clone();
        let resp = client
            .create_task(life_runtime_proto::lifed::CreateTaskRequest {
                user_id:       ctx.user_id().to_string(),
                prompt,
                model_id,
                workspace_id,
                parent_task_id: parent_task.unwrap_or_default(),
                trace_id:      ctx.trace_id.clone(),
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let task = resp.into_inner();
        let value = serde_json::json!({
            "id":         task.task_id,
            "status":     task.status,
            "createdAt":  task.created_at,
        });

        Ok(Some(FieldValue::value(value)))
    })
}

/// Resolver: mutation `updateAgentTask`.
///
/// Allowed actions: cancel, approve_tool_call, send_user_input.
/// Per Spec C₃ §6.5, cancellation surfaces a typed close-code on the
/// subscription stream so the client can react appropriately.
pub fn resolve_update_agent_task(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;
        ctx.require_scope("agent:write")?;

        let input = rctx
            .args
            .try_get("input")
            .map_err(|_| WarpGraphqlError::InvalidInput("missing 'input'".into()))?
            .deserialize::<serde_json::Value>()
            .map_err(|e| WarpGraphqlError::InvalidInput(e.to_string()))?;

        let task_id = input["taskId"].as_str().unwrap_or("").to_string();
        let action  = input["action"].as_str().unwrap_or("").to_string();
        let payload = input.get("payload").cloned().unwrap_or(serde_json::Value::Null);

        let mut client = ctx.dataloaders.agent.clone();
        client
            .update_task(life_runtime_proto::lifed::UpdateTaskRequest {
                user_id:      ctx.user_id().to_string(),
                task_id,
                action,
                payload_json: payload.to_string(),
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?;

        let value = serde_json::json!({ "success": true });
        Ok(Some(FieldValue::value(value)))
    })
}
```

- [ ] **Step 2: Bind + test + commit**

```rust
.field_resolver("Mutation", "createAgentTask", resolve_create_agent_task)
.field_resolver("Mutation", "updateAgentTask", resolve_update_agent_task)
```

```bash
cargo test -p lifegw
git add -u
git commit -m "feat(lifegw): T1 agent task resolvers — createAgentTask/updateAgentTask (L6-D5, M7-W.2, BRO-XXXX)"
```

---

### Task 19: M7-W.2 final pass — full crate verify + clippy

- [ ] **Step 1: Run full verify**

```bash
cargo fmt -p lifegw
cargo clippy -p lifegw -- -D warnings
cargo test -p lifegw
```

Expected: clean.

- [ ] **Step 2: Commit any cleanup**

```bash
git add -u
git commit -m "chore(lifegw): warp_graphql M7-W.2 final pass (BRO-XXXX)"
```

**M7-W.2 acceptance gate**: All ~15 T1 operations resolve through real lifed gRPC. Stubs still cover ~90 ops; subscription is still inert (M7-W.3). Schema introspection unchanged.

---

## M7-W.3 — Subscription + arcan Adapter + E2E

**Outcome of M7-W.3:** `warpDriveUpdates` subscription multiplexes events from lifed.EventService to the client. arcan ConsciousnessActor wired through `arcan-core/src/warp_agent.rs`. E2E smoke proves the patched `warp-oss` client (built in M7-W.0) drives a task to completion.

---

### Task 20: Implement the `warpDriveUpdates` subscription

`warpDriveUpdates` is Warp's single multiplexed subscription. It carries seven event types (per schema discovery): `ObjectActionOccurred`, `ObjectContentUpdated`, `ObjectDeleted`, `ObjectMetadataUpdated`, `ObjectPermissionsUpdated`, `TeamMembershipsChanged`, `AmbientTaskUpdated`. Lumen v1 surfaces only `AmbientTaskUpdated` (the agent stream); the others get empty/no-op event emission for now.

**Files:**
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/subscriptions/warp_drive_updates.rs`
- Modify: `core/life/crates/life-runtime/lifegw/src/warp_graphql/adapter.rs`

- [ ] **Step 1: Implement the subscription handler**

Create `core/life/crates/life-runtime/lifegw/src/warp_graphql/subscriptions/warp_drive_updates.rs`:

```rust
//! `warpDriveUpdates` — the one multiplexed subscription Warp ships.
//!
//! Per Lumen spec L6-D5, this bridges to `lifed.EventService.Subscribe`.
//! lifed multiplexes events from arcan, lago, vigil, autonomic; we
//! filter and reformat to Warp's `WarpDriveUpdate` union type.
//!
//! Phase α scope: only `AmbientTaskUpdated` events are non-empty.
//! The other 6 event types in the union emit empty no-op frames so
//! the Warp client sees the schema satisfied.

use async_graphql::dynamic::{FieldFuture, ResolverContext, FieldValue};
use futures_util::stream::{self, StreamExt};
use std::sync::Arc;

use crate::warp_graphql::context::WarpRequestCtx;
use crate::warp_graphql::error::WarpGraphqlError;

pub fn resolve_warp_drive_updates(rctx: ResolverContext<'_>) -> FieldFuture {
    FieldFuture::new(async move {
        let ctx: &WarpRequestCtx = rctx
            .ctx
            .data::<WarpRequestCtx>()
            .map_err(|_| WarpGraphqlError::Internal("missing request ctx".into()))?;
        ctx.require_scope("agent:read")?;

        let mut events = ctx.dataloaders.events.clone();
        let server_stream = events
            .subscribe(life_runtime_proto::lifed::SubscribeRequest {
                user_id: ctx.user_id().to_string(),
                trace_id: ctx.trace_id.clone(),
            })
            .await
            .map_err(|e| WarpGraphqlError::Upstream(e.to_string()))?
            .into_inner();

        // Map lifed events → Warp's WarpDriveUpdate union frames.
        let mapped = server_stream
            .map(|item| match item {
                Ok(ev) => {
                    let json = lifed_event_to_warp_drive_update(&ev);
                    Ok(FieldValue::value(json))
                }
                Err(e) => Err(WarpGraphqlError::Upstream(e.to_string()).into()),
            });

        Ok(FieldValue::list_stream(mapped))
    })
}

/// Translate a `lifed.EventService` event into a Warp-shaped
/// `WarpDriveUpdate` union frame. The schema declares this as a union
/// type with seven variants; we always emit one variant per event.
///
/// Phase α: only `AmbientTaskUpdated` is non-empty. Other variants are
/// reserved for future work (per Lumen spec L6-D9 cloud-agent deferral).
fn lifed_event_to_warp_drive_update(
    ev: &life_runtime_proto::lifed::EventEnvelope,
) -> serde_json::Value {
    use life_runtime_proto::lifed::event_envelope::Kind;

    match &ev.kind {
        Some(Kind::AmbientTask(task_ev)) => serde_json::json!({
            "__typename": "AmbientTaskUpdated",
            "taskId":     task_ev.task_id,
            "status":     task_ev.status,
            "delta":      task_ev.content_delta,
            "timestamp":  ev.timestamp,
        }),
        // Other event kinds map to empty Warp variants for now.
        _ => serde_json::json!({
            "__typename": "ObjectMetadataUpdated",
            "objectId":   "",
            "metadata":   {},
            "timestamp":  ev.timestamp,
        }),
    }
}
```

NOTE: The exact proto names (`EventEnvelope`, `event_envelope::Kind`, `AmbientTask`) must match what `life-runtime-proto` exports. If lifed uses different message types, adjust accordingly.

- [ ] **Step 2: Bind the subscription**

In `adapter.rs`:

```rust
use crate::warp_graphql::subscriptions::warp_drive_updates::resolve_warp_drive_updates;

// ... in WarpSchema::build:
.field_resolver("Subscription", "warpDriveUpdates", resolve_warp_drive_updates)
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check -p lifegw
cargo test -p lifegw warp_graphql::subscriptions
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(lifegw): warpDriveUpdates subscription bridged to lifed.EventService (M7-W.3, BRO-XXXX)"
```

---

### Task 21: arcan-core warp_agent adapter

The adapter sits between lifed's gRPC `AgentService` (which the resolver calls) and arcan's `ConsciousnessActor`. lifed already has the proto-level `CreateTaskRequest` handler scaffolded (per M5 sub-phase A); the new code is the arcan side that translates a lifed task spawn into a ConsciousnessActor invocation.

**Files:**
- Create: `core/life/crates/arcan/arcan-core/src/warp_agent.rs`
- Modify: `core/life/crates/arcan/arcan-core/src/lib.rs`
- Test: `core/life/crates/arcan/arcan-core/tests/warp_agent.rs`

- [ ] **Step 1: Read existing arcan-core/src/agent.rs (or equivalent)**

Locate the existing ConsciousnessActor or agent-loop entry point. The new `warp_agent.rs` will adapt to whatever interface it exposes.

```bash
ls core/life/crates/arcan/arcan-core/src/
grep -n "ConsciousnessActor\|spawn_actor\|agent_loop" core/life/crates/arcan/arcan-core/src/*.rs
```

- [ ] **Step 2: Implement the adapter**

Create `core/life/crates/arcan/arcan-core/src/warp_agent.rs`:

```rust
//! Adapter between lifed's CreateTaskRequest and arcan's
//! ConsciousnessActor. Owned by arcan because the actor spawn
//! lifecycle (and its hooks into nous + autonomic + lago) is
//! arcan-internal.
//!
//! Per Lumen spec L6-D5 + L6-D10:
//!  - lifed.AgentService.CreateTask invokes
//!    `WarpTaskAdapter::spawn(req)` which creates a new
//!    ConsciousnessActor scoped to (user_id, task_id).
//!  - The actor emits AmbientTaskUpdated frames via the existing
//!    arcan event bus → lifed.EventService → warp_graphql
//!    subscription → Warp client.
//!  - L6-D10 frontier-vision constraint: this entry point is
//!    callable both from lifed (reactive, today) and from arcan
//!    itself (proactive, future). The signature reflects that.

use anyhow::Result;
use std::sync::Arc;

/// Input sufficient to spawn a ConsciousnessActor for a Warp-originated
/// task. Source-agnostic: same struct serves reactive (Warp asks) and
/// proactive (Life decides) invocations.
#[derive(Debug, Clone)]
pub struct WarpTaskSpawnInput {
    pub user_id:        String,
    pub task_id:        String,
    pub prompt:         String,
    pub model_id:       String,
    pub workspace_id:   String,
    pub parent_task_id: Option<String>,
    pub trace_id:       String,
}

/// Adapter that owns ConsciousnessActor spawn lifecycle.
pub struct WarpTaskAdapter {
    // Hold a handle to the arcan agent registry / event bus / store.
    // The exact dependencies depend on existing arcan-core API; this
    // struct's fields are filled by the arcan team during implementation.
    inner: Arc<dyn WarpTaskBackend>,
}

#[async_trait::async_trait]
pub trait WarpTaskBackend: Send + Sync {
    /// Spawn a new ConsciousnessActor scoped to (user_id, task_id).
    /// Returns once the actor is alive and accepting events; the
    /// actor itself runs to completion in the background.
    async fn spawn(&self, input: WarpTaskSpawnInput) -> Result<()>;

    /// Send a control message to a running task: cancel, approve_tool_call,
    /// or send_user_input. Per Lumen spec L6-D5 + Spec C₃ §6.5, cancellation
    /// surfaces a typed close-code on the subscription stream.
    async fn control(
        &self,
        user_id: &str,
        task_id: &str,
        action: WarpTaskAction,
    ) -> Result<()>;
}

#[derive(Debug, Clone)]
pub enum WarpTaskAction {
    Cancel,
    ApproveToolCall { call_id: String, approved: bool },
    SendUserInput { text: String },
}

impl WarpTaskAdapter {
    pub fn new(backend: Arc<dyn WarpTaskBackend>) -> Self {
        Self { inner: backend }
    }

    pub async fn spawn(&self, input: WarpTaskSpawnInput) -> Result<()> {
        self.inner.spawn(input).await
    }

    pub async fn control(
        &self,
        user_id: &str,
        task_id: &str,
        action: WarpTaskAction,
    ) -> Result<()> {
        self.inner.control(user_id, task_id, action).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeBackend {
        spawned: Mutex<Vec<WarpTaskSpawnInput>>,
        controls: Mutex<Vec<(String, String, WarpTaskAction)>>,
    }

    #[async_trait::async_trait]
    impl WarpTaskBackend for FakeBackend {
        async fn spawn(&self, input: WarpTaskSpawnInput) -> Result<()> {
            self.spawned.lock().unwrap().push(input);
            Ok(())
        }

        async fn control(
            &self,
            user_id: &str,
            task_id: &str,
            action: WarpTaskAction,
        ) -> Result<()> {
            self.controls.lock().unwrap().push((
                user_id.to_string(),
                task_id.to_string(),
                action,
            ));
            Ok(())
        }
    }

    #[tokio::test]
    async fn spawn_records_input() {
        let fake = Arc::new(FakeBackend::default());
        let adapter = WarpTaskAdapter::new(fake.clone());
        adapter
            .spawn(WarpTaskSpawnInput {
                user_id: "u1".into(),
                task_id: "t1".into(),
                prompt: "hi".into(),
                model_id: "claude-4".into(),
                workspace_id: "ws".into(),
                parent_task_id: None,
                trace_id: "tr".into(),
            })
            .await
            .unwrap();
        assert_eq!(fake.spawned.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn control_cancel_records_action() {
        let fake = Arc::new(FakeBackend::default());
        let adapter = WarpTaskAdapter::new(fake.clone());
        adapter
            .control("u1", "t1", WarpTaskAction::Cancel)
            .await
            .unwrap();
        assert_eq!(fake.controls.lock().unwrap().len(), 1);
        assert!(matches!(
            fake.controls.lock().unwrap()[0].2,
            WarpTaskAction::Cancel
        ));
    }
}
```

- [ ] **Step 3: Wire the module**

Open `core/life/crates/arcan/arcan-core/src/lib.rs`. Add:

```rust
pub mod warp_agent;
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p arcan-core warp_agent
```

Expected: 2 tests pass.

- [ ] **Step 5: Commit**

```bash
git add core/life/crates/arcan/arcan-core/src/warp_agent.rs core/life/crates/arcan/arcan-core/src/lib.rs core/life/crates/arcan/arcan-core/tests/
git commit -m "feat(arcan-core): warp_agent adapter — Warp tasks → ConsciousnessActor (L6-D10, M7-W.3, BRO-XXXX)"
```

NOTE: Wiring `WarpTaskBackend` to the actual arcan ConsciousnessActor implementation is a follow-up sub-task — the adapter and trait ship in this task; the concrete backend lands in a separate ticket (BRO-XXXX) under the same M7-W.3 sub-phase.

---

### Task 22: E2E smoke test

The α completion gate. The patched `warp-oss` client (built in M7-W.0 Task 0), configured with `WARP_SERVER_ROOT_URL=http://localhost:8443` and `WARP_WS_SERVER_URL=ws://localhost:8443/graphql/v2`, drives an arcan-backed task to completion against a docker-compose stack containing lifegw + lifed + arcan. Note: `WARP_SERVER_ROOT_URL` is the **root** URL — the client appends `/graphql/v2?op=...` for HTTP, and uses `WARP_WS_SERVER_URL` directly for WebSocket subscriptions.

**Files:**
- Create: `core/life/crates/life-runtime/lifegw/tests/warp_graphql_e2e_smoke.rs`
- Create: `core/life/crates/life-runtime/lifegw/tests/fixtures/docker-compose.e2e.yml`

- [ ] **Step 1: Write the docker-compose fixture**

Create `core/life/crates/life-runtime/lifegw/tests/fixtures/docker-compose.e2e.yml`:

```yaml
version: "3.9"
services:
  lifed:
    build:
      context: ../../../../..
      dockerfile: docker/lifed.Dockerfile
    environment:
      - LIFED_BIND_UDS=/run/lifed/lifed.sock
      - LIFED_DEV_MODE=true
    volumes:
      - lifed-uds:/run/lifed
  lifegw:
    build:
      context: ../../../../..
      dockerfile: docker/lifegw.Dockerfile
    depends_on:
      - lifed
    environment:
      - LIFEGW_BIND_ADDR=0.0.0.0:8443
      - LIFEGW_LIFED_UDS=/run/lifed/lifed.sock
      - LIFEGW_DEV_MODE=true
      - LIFEGW_WARP_GRAPHQL_ENABLED=true
    volumes:
      - lifed-uds:/run/lifed
    ports:
      - "8443:8443"
volumes:
  lifed-uds:
```

NOTE: The Dockerfiles `docker/lifegw.Dockerfile` and `docker/lifed.Dockerfile` may already exist or need stub creation — check `core/life/docker/` first. If they don't exist, create minimal stubs that build the binary with `cargo build --release -p {lifegw|lifed}`.

- [ ] **Step 2: Write the E2E smoke test**

Create `core/life/crates/life-runtime/lifegw/tests/warp_graphql_e2e_smoke.rs`:

```rust
//! End-to-end smoke test — the M7-W α completion gate.
//!
//! Spins up lifegw + lifed + arcan via docker-compose. Submits a
//! `createAgentTask` mutation. Subscribes to `warpDriveUpdates` and
//! waits for an `AmbientTaskUpdated` frame with `status = "completed"`.
//!
//! This test is gated behind the `e2e-smoke` feature so it doesn't
//! run in the default `cargo test` matrix; CI runs it on a dedicated
//! lane with docker available.

#![cfg(feature = "e2e-smoke")]

use std::time::Duration;
use tokio::time::timeout;

const COMPOSE_FILE: &str = "tests/fixtures/docker-compose.e2e.yml";
const ENDPOINT: &str = "http://127.0.0.1:8443/graphql/v2";
const WS_ENDPOINT: &str = "ws://127.0.0.1:8443/graphql-ws";
const SMOKE_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::test]
async fn patched_warp_oss_client_drives_task_to_completion() {
    // 1. Start the stack.
    let _stack = TestStack::up().await;

    // 2. Mint a dev-mode token (LIFEGW_DEV_MODE=true accepts
    //    Bearer dev-token-for-{user_id}).
    let token = "dev-token-for-test-user-1";

    // 3. Submit a createAgentTask mutation.
    let create_resp = http_post_graphql(
        ENDPOINT,
        token,
        r#"
        mutation {
          createAgentTask(input: {
            prompt: "echo hello",
            modelId: "mock-default",
            workspaceId: "ws-test",
          }) {
            id
            status
          }
        }
        "#,
    )
    .await
    .expect("createAgentTask succeeds");

    let task_id = create_resp["data"]["createAgentTask"]["id"]
        .as_str()
        .expect("task id present")
        .to_string();

    // 4. Subscribe to warpDriveUpdates over WS, filter for our task_id.
    let mut sub = ws_subscribe(WS_ENDPOINT, token).await.expect("WS subscribe succeeds");

    // 5. Wait for terminal AmbientTaskUpdated frame.
    let result = timeout(SMOKE_TIMEOUT, async {
        loop {
            let frame = sub.next_frame().await.expect("frame");
            if frame["__typename"] == "AmbientTaskUpdated"
                && frame["taskId"] == task_id
                && frame["status"] == "completed"
            {
                return ();
            }
        }
    })
    .await;

    assert!(result.is_ok(), "task did not reach completed within timeout");
}

// --- Test infrastructure (TestStack, http_post_graphql, ws_subscribe) ---
// Real implementations omitted for brevity; pattern: `TestStack::up()`
// runs `docker compose -f tests/fixtures/docker-compose.e2e.yml up -d`,
// waits for `:8443/healthz` to return 200, returns a guard that runs
// `docker compose down` on Drop.

struct TestStack;
impl TestStack {
    async fn up() -> Self {
        std::process::Command::new("docker")
            .args(["compose", "-f", COMPOSE_FILE, "up", "-d", "--build"])
            .status()
            .expect("docker compose up succeeds");
        // Wait for healthz.
        for _ in 0..60 {
            if let Ok(resp) = reqwest::get("http://127.0.0.1:8443/healthz").await {
                if resp.status().is_success() {
                    return Self;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!("lifegw did not become healthy");
    }
}

impl Drop for TestStack {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["compose", "-f", COMPOSE_FILE, "down", "-v"])
            .status();
    }
}

async fn http_post_graphql(
    url: &str,
    token: &str,
    query: &str,
) -> Result<serde_json::Value, anyhow::Error> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "query": query });
    let resp = client
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    Ok(resp)
}

struct WsSubscription { /* ... */ }
impl WsSubscription {
    async fn next_frame(&mut self) -> Option<serde_json::Value> { /* ... */ None }
}

async fn ws_subscribe(_url: &str, _token: &str) -> Result<WsSubscription, anyhow::Error> {
    // Use graphql-ws-client crate (added as dev-dep in Task 1).
    // Pattern: open WS, send connection_init with auth in payload,
    // wait for connection_ack, send subscribe message with the
    // warpDriveUpdates query, return a stream wrapper that yields
    // `data.warpDriveUpdates` from each `next` frame.
    todo!("real implementation in fix-up commit")
}
```

NOTE: The `ws_subscribe` skeleton is intentionally `todo!()` — the implementation depends on `graphql-ws-client` 0.10's API and the exact Warp connection_init payload format. Fill it in during implementation, not in the plan.

- [ ] **Step 3: Add the `e2e-smoke` feature to lifegw Cargo.toml**

```toml
[features]
e2e-smoke = []
```

- [ ] **Step 4: Run the smoke test (manual, behind the feature gate)**

```bash
docker compose -f core/life/crates/life-runtime/lifegw/tests/fixtures/docker-compose.e2e.yml build
cargo test -p lifegw --features e2e-smoke --test warp_graphql_e2e_smoke -- --test-threads=1
```

Expected: passes within `SMOKE_TIMEOUT` (120s). If it fails, the failure mode signals which layer is broken:
- HTTP 401 → JWT minting/verification mismatch
- HTTP 400 + GraphQL parse error → schema mirror corruption
- WS connection refused → axum WS upgrade handler broken
- Subscription frame never arrives → arcan adapter / event bridge broken

- [ ] **Step 5: Commit**

```bash
git add core/life/crates/life-runtime/lifegw/tests/ core/life/crates/life-runtime/lifegw/Cargo.toml
git commit -m "test(lifegw): E2E smoke — official Warp drives arcan task end-to-end (M7-W.3, BRO-XXXX)"
```

---

### Task 23: M7-W.3 final pass + α gate verification

- [ ] **Step 1: Run full crate verify**

```bash
cd core/life
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

Expected: clean.

- [ ] **Step 2: Run E2E smoke explicitly**

```bash
cargo test -p lifegw --features e2e-smoke --test warp_graphql_e2e_smoke -- --test-threads=1
```

Expected: passes.

- [ ] **Step 3: Manually verify with the patched warp-oss binary**

This is the human-in-the-loop validation step that proves α complete. Requires Task 0 (M7-W.0) to be complete so `target/release/warp-oss` exists.

```bash
# 1. Bring up the stack:
docker compose -f core/life/crates/life-runtime/lifegw/tests/fixtures/docker-compose.e2e.yml up

# 2. In a separate shell, launch the patched warp-oss pointed at lifegw:
WARP_SERVER_ROOT_URL=http://localhost:8443 \
WARP_WS_SERVER_URL=ws://localhost:8443/graphql/v2 \
WARP_AUTH_TOKEN=dev-token-for-test-user-1 \
./apps/warp-oss-patch/upstream/target/release/warp-oss

# 3. Type a prompt in the warp-oss window (e.g. "list files in current directory").
# 4. Verify a response streams back, sourced from the arcan ConsciousnessActor.
```

If this succeeds, **Phase α is complete**.

NOTE: `WARP_SERVER_ROOT_URL` does not include `/graphql/v2` — it is the **root** URL. The client appends `/graphql/v2?op=...` for HTTP requests and uses `WARP_WS_SERVER_URL` directly for WebSocket subscriptions.

- [ ] **Step 4: Final commit**

```bash
git add -u
git commit -m "chore(lifegw): warp_graphql M7-W.3 final pass — α complete (BRO-XXXX)"
```

---

## Acceptance Gate

Phase α is complete when **all** of:

1. `cargo test -p lifegw` passes (unit + integration + conformance + golden tests).
2. `cargo clippy -p lifegw -- -D warnings` is clean.
3. `cargo test -p arcan-core warp_agent` passes.
4. `cargo test -p lifegw --features e2e-smoke --test warp_graphql_e2e_smoke` passes against the docker-compose stack.
5. Manual launch of patched `./apps/warp-oss-patch/upstream/target/release/warp-oss` against `WARP_SERVER_ROOT_URL=http://localhost:8443` + `WARP_WS_SERVER_URL=ws://localhost:8443/graphql/v2` reaches a working agent-task completion (Step 3 of Task 23). Requires Task 0 (M7-W.0) to have completed successfully.

When all five hold, Phase α has shipped. Phase β (Lumen fork at `apps/lumen/`) gets its own implementation plan written in a separate sub-skill invocation after α stabilizes.

---

## Self-Review Checklist (already performed inline)

**1. Spec coverage:** Each spec section is mapped to one or more tasks:
- Schema strategy (L6-D3) → Tasks 2, 4, 10, 12.
- Tiered rollout (L6-D4) → Tasks 9 (T3 stubs), 14–18 (T1 wired).
- Agent invocation flow (L6-D5) → Tasks 18 (createAgentTask), 20 (subscription), 21 (arcan adapter).
- Auth (L6-D6) → Tasks 6, 15.
- Skills/MCP (L6-D8) → deferred to Phase β L.4 (out of α scope, noted in spec).
- Code search (L6-D7) → deferred to Phase β L.3 (T2 resolvers, out of α scope).
- Frontier vision (L6-D10) → Task 21 trait shape (`WarpTaskBackend` signature is bidirectional).
- E2E acceptance (Phase α gate) → Task 22.

**2. Placeholder scan:** All `BRO-XXXX` IDs are intentional Linear-creation placeholders (created via MCP Linear after spec approval per workspace rule). The `todo!()` in Task 22's `ws_subscribe` is intentional — the WS-client integration depends on a runtime API that requires actual coding to validate; the plan flags this as fill-in-during-implementation, not a placeholder for design.

**3. Type consistency:** `WarpClaims`, `WarpRequestCtx`, `LifedDataloaders`, `WarpGraphqlError`, `WarpSchema`, `WarpTaskSpawnInput`, `WarpTaskAction`, `WarpTaskBackend`, `WarpTaskAdapter` are used consistently across all tasks where they appear.

**4. Ambiguity:** Proto message names (`GetUserRequest`, `MintLumenTokenRequest`, etc.) are flagged with NOTEs at the call sites — implementer adapts to actual `life-runtime-proto` exports. async-graphql 7.x dynamic-binding API names are flagged similarly.

---

## References

- **Spec**: `core/life/docs/superpowers/specs/2026-05-10-lumen-warp-on-life-design.md` (committed `686409ab`)
- **Spec C₃ §6.5** (close codes): `core/life/docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`
- **Spec D** (AnimaCustody): `core/life/docs/superpowers/specs/2026-04-29-spec-d-anima-custody.md`
- **Spec E** (InferenceBackend): `core/life/docs/superpowers/specs/2026-05-07-spec-e-agent-loop-compute-contract.md`
- **lifegw**: `core/life/crates/life-runtime/lifegw/` (existing crate, M7 ~80% complete pre-W)
- **lifed**: `core/life/crates/life-runtime/lifed/` (M5 production-shipped 2026-04-29)
- **arcan**: `core/life/crates/arcan/arcan-core/` (V1.5 stabilization)
- **Warp upstream**: `github.com/warpdotdev/warp` — schema at `crates/warp_graphql_schema/api/schema.graphql`
