# Tiered Sandbox Default — BashTool via SandboxProvider

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route BashTool through a tiered sandbox provider chain (Vercel → bwrap → subprocess fallback) so shell commands execute in isolated environments by default, with bwrap installed on Railway for namespace-level isolation.

**Architecture:** Replace `LocalCommandRunner` with `SandboxCommandRunner` in `main.rs` tool registration. Add `build_sandbox_provider_with_fallback()` to `sandbox_router.rs` that auto-detects the best available provider. Move sandbox provider creation before tool registration (it's sync-safe). Install `bubblewrap` in the Dockerfile runtime stage. Set `VERCEL_TOKEN` on Railway for Vercel Sandbox as preferred tier.

**Tech Stack:** Rust 2024, arcan-sandbox, arcan-provider-bubblewrap, arcan-provider-vercel, arcan-praxis (SandboxCommandRunner), Railway CLI, bubblewrap (bwrap)

---

## File Map

| File | Action | Purpose |
|------|--------|---------|
| `arcan/crates/arcan/src/sandbox_router.rs` | Modify | Add `build_sandbox_provider_with_fallback()` |
| `arcan/crates/arcan/src/main.rs` | Modify | Move provider init before tool reg, swap runner |
| `arcan/Dockerfile` | Modify | Install `bubblewrap` in runtime stage |
| Railway env vars | Set | `VERCEL_TOKEN`, `VERCEL_PROJECT_ID` |

---

### Task 1: Add tiered fallback provider builder

**Files:**
- Modify: `arcan/crates/arcan/src/sandbox_router.rs`

- [ ] **Step 1: Add `build_sandbox_provider_with_fallback` function**

Add this function after the existing `build_sandbox_provider`:

```rust
/// Auto-detect the best available sandbox provider using a tiered fallback chain:
///
/// 1. **Vercel Sandbox** (Firecracker microVM) — if `VERCEL_TOKEN` is set
/// 2. **Bubblewrap** (Linux namespaces) — if `bwrap` binary is in PATH
/// 3. **Subprocess fallback** — plain process in per-sandbox workspace dir
///
/// Always returns a provider — never `None`. The chain guarantees at least
/// subprocess-level isolation (per-session workspace directory).
///
/// To force a specific backend, set `ARCAN_SANDBOX_BACKEND` (checked first).
pub fn build_sandbox_provider_with_fallback() -> Arc<dyn SandboxProvider> {
    // Explicit override takes precedence.
    let explicit = std::env::var("ARCAN_SANDBOX_BACKEND").unwrap_or_default();
    if !explicit.is_empty() && explicit != "auto" {
        if let Some(provider) = build_sandbox_provider() {
            return provider;
        }
        // Explicit backend failed — fall through to auto-detect.
        tracing::warn!(
            backend = %explicit,
            "explicit ARCAN_SANDBOX_BACKEND failed, falling through to auto-detect"
        );
    }

    // Tier 1: Vercel Sandbox (requires VERCEL_TOKEN)
    if std::env::var("VERCEL_TOKEN").is_ok() {
        match arcan_provider_vercel::VercelSandboxProvider::from_env() {
            Ok(p) => {
                tracing::info!("Sandbox: Vercel (Firecracker microVM)");
                return Arc::new(p);
            }
            Err(e) => {
                tracing::warn!(error = %e, "Vercel Sandbox unavailable, trying bwrap");
            }
        }
    }

    // Tier 2/3: Bubblewrap (auto-detects bwrap binary, falls back to subprocess)
    let p = arcan_provider_bubblewrap::BubblewrapProvider::from_env();
    if p.use_bwrap {
        tracing::info!("Sandbox: Bubblewrap (Linux namespace isolation)");
    } else {
        tracing::info!("Sandbox: subprocess fallback (workspace directory isolation)");
    }
    Arc::new(p)
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd arcan && cargo check -p arcan
```
Expected: compiles clean.

- [ ] **Step 3: Commit**

```bash
git add crates/arcan/src/sandbox_router.rs
git commit -m "feat(sandbox): tiered fallback provider — Vercel → bwrap → subprocess"
```

---

### Task 2: Wire BashTool through SandboxCommandRunner

**Files:**
- Modify: `arcan/crates/arcan/src/main.rs` (lines ~435-436 and ~640-643)

- [ ] **Step 1: Move sandbox provider creation before tool registration**

In `run_serve()`, find the current tool registration block (around line 424-436):

```rust
    // --- Tools (Praxis canonical implementations, bridged into Arcan) ---
    let mut registry = ToolRegistry::default();
    // ... file tool registrations ...
    let runner = Box::new(LocalCommandRunner);
    registry.register(PraxisToolBridge::new(BashTool::new(sandbox_policy, runner)));
```

Insert the sandbox provider creation BEFORE the tool registration, and replace `LocalCommandRunner`:

```rust
    // --- Sandbox provider (tiered: Vercel → bwrap → subprocess) ---
    let sandbox_provider = crate::sandbox_router::build_sandbox_provider_with_fallback();

    // --- Tools (Praxis canonical implementations, bridged into Arcan) ---
    let mut registry = ToolRegistry::default();
    // ... file tool registrations stay unchanged ...

    // BashTool routes through SandboxCommandRunner for per-session isolation.
    let runner: Box<dyn praxis_core::sandbox::CommandRunner> =
        Box::new(arcan_praxis::SandboxCommandRunner::new(sandbox_provider.clone()));
    registry.register(PraxisToolBridge::new(BashTool::new(sandbox_policy, runner)));
```

- [ ] **Step 2: Remove the old `build_sandbox_provider()` call from inside block_on**

Find the existing sandbox provider creation inside the `tokio_runtime.block_on(async move {` block (around line 640-643):

```rust
        // --- HTTP Server ---
        // BRO-250: Build sandbox provider from ARCAN_SANDBOX_BACKEND env var.
        let sandbox_provider = crate::sandbox_router::build_sandbox_provider();
        let sandbox_store = Arc::new(arcan_sandbox::InMemorySessionStore::new());
```

Replace with:

```rust
        // --- HTTP Server ---
        // Sandbox provider was created before tool registration (tiered fallback).
        // Wrap in Option for the lifecycle observer (which needs Arc<dyn SandboxProvider>).
        let sandbox_store = Arc::new(arcan_sandbox::InMemorySessionStore::new());
```

And update the lifecycle observer block that follows (around line 653-664) to use `sandbox_provider` directly instead of checking `if let Some(ref provider)`:

```rust
        // Register lifecycle observer for session cleanup.
        {
            use aios_protocol::SubscriptionTier;
            use arcan_aios_adapters::SandboxLifecycleObserver;
            run_observers.push(Arc::new(SandboxLifecycleObserver::new(
                sandbox_provider.clone(),
                Arc::clone(&sandbox_store),
                SubscriptionTier::Anonymous,
            )));
        }
```

- [ ] **Step 3: Add the `arcan_praxis` import if not present**

Check the top of main.rs for `use arcan_praxis`. If missing, the `SandboxCommandRunner` path `arcan_praxis::SandboxCommandRunner` should resolve via crate name. Verify with `cargo check`.

- [ ] **Step 4: Verify compilation**

```bash
cd arcan && cargo fmt && cargo check -p arcan
```
Expected: compiles clean. No unused imports for `LocalCommandRunner` — keep it imported as it may be used elsewhere; if clippy warns, remove it.

- [ ] **Step 5: Run workspace tests**

```bash
cd arcan && cargo test -p arcan-praxis && cargo test -p arcan-provider-bubblewrap
```
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/arcan/src/main.rs
git commit -m "feat(sandbox): route BashTool through SandboxCommandRunner (BRO-259)

BashTool now uses SandboxCommandRunner → tiered SandboxProvider instead
of LocalCommandRunner → direct process exec. Every bash command runs in
a per-invocation sandbox workspace with provider-specific isolation:
  - Vercel: Firecracker microVM (full VM isolation)
  - bwrap: Linux namespaces (network blocked, FS bind-mounted)
  - fallback: plain subprocess in isolated workspace directory"
```

---

### Task 3: Install bubblewrap in Dockerfile

**Files:**
- Modify: `arcan/Dockerfile` (runtime stage, around line 32-34)

- [ ] **Step 1: Add bubblewrap to apt-get install**

Find the runtime stage `apt-get install` line:

```dockerfile
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*
```

Add `bubblewrap`:

```dockerfile
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl bubblewrap && \
    rm -rf /var/lib/apt/lists/*
```

- [ ] **Step 2: Bump cache bust**

Change `ARG CACHE_BUST=20260327c` to `ARG CACHE_BUST=20260327d`.

- [ ] **Step 3: Commit**

```bash
git add Dockerfile
git commit -m "feat(docker): install bubblewrap for Linux namespace sandbox isolation"
```

---

### Task 4: Set Railway environment variables

- [ ] **Step 1: Set VERCEL_TOKEN for Vercel Sandbox**

```bash
cd /Users/broomva/broomva/core/life

# Get Vercel token from broomva.tech project (or use existing)
# Set on Railway arcan service
railway variables set "VERCEL_TOKEN=$(grep VERCEL_TOKEN /Users/broomva/broomva/broomva.tech/.env.local 2>/dev/null | cut -d= -f2 || echo '')"

# Set project ID for sandbox scoping
railway variables set "VERCEL_PROJECT_ID=<broomva-tech-project-id>"
```

If Vercel token is not available, skip — bwrap will be the default. The tiered chain handles this gracefully.

- [ ] **Step 2: Remove explicit ARCAN_SANDBOX_BACKEND if set**

```bash
railway variables delete ARCAN_SANDBOX_BACKEND 2>/dev/null || true
```

The tiered auto-detect replaces the explicit env var.

- [ ] **Step 3: Verify env vars**

```bash
railway variables | grep -i -E "VERCEL_TOKEN|VERCEL_PROJECT|SANDBOX"
```

---

### Task 5: Push, deploy, and verify

- [ ] **Step 1: Push all commits**

```bash
cd /Users/broomva/broomva/core/life/arcan
git push origin main
```

- [ ] **Step 2: Wait for Railway build**

```bash
sleep 360 && railway deployment list --service arcan --limit 1 --json | python3 -c "import json,sys; d=json.load(sys.stdin)[0]; print(d['status'])"
```
Expected: `SUCCESS`

- [ ] **Step 3: Verify sandbox provider in startup logs**

```bash
railway logs --latest --lines 50 --json | python3 -c "
import json, sys
for line in sys.stdin:
    try:
        d = json.loads(line.strip())
        fields = d.get('fields', {})
        msg = fields.get('message', '') or d.get('message', '')
        if 'sandbox' in msg.lower() or 'Sandbox' in msg:
            print(msg.strip())
    except: pass
"
```
Expected: One of:
- `Sandbox: Vercel (Firecracker microVM)` — if VERCEL_TOKEN worked
- `Sandbox: Bubblewrap (Linux namespace isolation)` — if bwrap installed
- `Sandbox: subprocess fallback (workspace directory isolation)` — minimum

- [ ] **Step 4: E2E test via broomva.tech chat**

Send a message that triggers bash tool use:
```
Run `echo "sandbox-test-$(date +%s)" > /tmp/test.txt && cat /tmp/test.txt` and show the output
```

Verify the response shows the command executed successfully. Check Langfuse for the trace showing sandbox provider usage.
