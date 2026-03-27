# Fix Arcan Railway Deployment Crash

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the Arcan Railway deployment crash-loop (Tokio runtime panic) and the subsequent build failure (unresolved SubscriptionTier import).

**Architecture:** Two independent fixes: (1) Make `RemoteLagoJournal` defer `reqwest::Client` creation to first async use via `tokio::sync::OnceCell`, avoiding the `hyper-util` panic when constructed in sync context. (2) Add a Docker cache-bust ARG to the Dockerfile so sibling repo clones are never stale.

**Tech Stack:** Rust 2024 Edition, reqwest 0.12, tokio, hyper-util, Docker

---

## Task 1: Fix `RemoteLagoJournal` — Lazy `reqwest::Client` Initialization

**Files:**
- Modify: `arcan/crates/arcan-lago/src/remote_journal.rs` (lines 50-68)
- Test: existing tests + new unit test in same file

### Root Cause

`RemoteLagoJournal::new()` calls `reqwest::Client::new()` which in reqwest 0.12+ creates a `hyper-util::TokioIo` connection pool requiring an active Tokio reactor. But `new()` is called at `main.rs:377` in sync context, 250 lines before the Tokio runtime is built at line 624. Every Railway deployment since March 26 21:49 UTC has been crash-looping because `LAGO_URL` is set in the Railway environment.

### Steps

- [ ] **Step 1: Write a test that reproduces the panic**

Add to `arcan/crates/arcan-lago/src/remote_journal.rs` at the bottom of the `#[cfg(test)]` module (or create one if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_does_not_require_tokio_runtime() {
        // This must succeed in a plain sync context (no #[tokio::test]).
        // Before the fix, this panics with:
        //   "there is no reactor running, must be called from the context of a Tokio 1.x runtime"
        let _journal = RemoteLagoJournal::new("http://localhost:9999");
    }
}
```

- [ ] **Step 2: Run the test to confirm it fails**

Run from `arcan/`:
```bash
cargo test -p arcan-lago -- tests::new_does_not_require_tokio_runtime --nocapture 2>&1
```
Expected: FAIL — panic at `hyper-util` TokioIo.

- [ ] **Step 3: Implement lazy client via `tokio::sync::OnceCell`**

Replace the struct and `impl` block in `remote_journal.rs`:

**Before (lines 50-64):**
```rust
pub struct RemoteLagoJournal {
    client: Arc<Client>,
    base_url: String,
}

impl RemoteLagoJournal {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Arc::new(Client::new()),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1{}", self.base_url, path)
    }
```

**After:**
```rust
pub struct RemoteLagoJournal {
    client: tokio::sync::OnceCell<Client>,
    base_url: String,
}

impl RemoteLagoJournal {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: tokio::sync::OnceCell::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Returns a shared `reqwest::Client`, creating it on first call.
    ///
    /// The client is created lazily because `reqwest::Client::new()` (v0.12+)
    /// requires an active Tokio reactor (`hyper-util::TokioIo`). By deferring
    /// creation to the first `.await` we guarantee we are inside the runtime.
    async fn client(&self) -> &Client {
        self.client.get_or_init(|| async { Client::new() }).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1{}", self.base_url, path)
    }
```

- [ ] **Step 4: Update all `self.client` usages to `self.client().await`**

Every method in `RemoteLagoJournal` that currently accesses `self.client.get(...)` or `self.client.post(...)` must change to `self.client().await.get(...)` / `self.client().await.post(...)`.

Search for all occurrences of `self.client.` (excluding `self.client()`) in the file and replace with `self.client().await.`.

- [ ] **Step 5: Run the sync test to confirm it passes**

```bash
cargo test -p arcan-lago -- tests::new_does_not_require_tokio_runtime --nocapture 2>&1
```
Expected: PASS (no panic — `Client::new()` is no longer called in the constructor).

- [ ] **Step 6: Run the full arcan-lago test suite**

```bash
cd arcan && cargo test -p arcan-lago 2>&1
```
Expected: all tests pass.

- [ ] **Step 7: Run the full workspace test suite**

```bash
cd arcan && cargo test --workspace 2>&1
```
Expected: all tests pass.

- [ ] **Step 8: Format and lint**

```bash
cd arcan && cargo fmt && cargo clippy --workspace 2>&1
```
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add crates/arcan-lago/src/remote_journal.rs
git commit -m "fix(arcan-lago): lazy reqwest::Client in RemoteLagoJournal — fixes Tokio runtime panic

reqwest 0.12+ creates a hyper-util TokioIo connection pool in Client::new(),
which panics if no Tokio reactor is running. RemoteLagoJournal::new() was
called in sync context (main.rs:377) before the runtime (main.rs:624).

Switch to tokio::sync::OnceCell so the Client is created on first .await,
guaranteed to be inside the runtime. Every Railway deploy since 2026-03-26
21:49 UTC has been crash-looping because of this."
```

---

## Task 2: Fix Docker Build — Cache-Bust Sibling Repo Clones

**Files:**
- Modify: `arcan/Dockerfile` (lines 3, 12-20)

### Root Cause

The Dockerfile uses `git clone --depth 1` for sibling repos (aiOS, lago, etc.). Docker layer caching can serve a stale clone even after new commits are pushed to those repos. The `SubscriptionTier` build failure was caused by the aiOS clone not having the latest commit.

### Steps

- [ ] **Step 1: Add cache-bust ARG to Dockerfile**

In `arcan/Dockerfile`, after the `FROM rust:latest AS builder` line and before `WORKDIR`, add:

```dockerfile
# Bust Docker cache for sibling repo clones.
# Railway passes a unique value per build; locally use:
#   docker build --build-arg CACHE_BUST=$(date +%s) .
ARG CACHE_BUST=0
```

Then prefix the `RUN git clone` block so the cache-bust invalidates it:

```dockerfile
RUN echo "cache-bust: ${CACHE_BUST}" && \
    git clone --depth 1 https://github.com/broomva/aiOS.git ../aiOS && \
    git clone --depth 1 https://github.com/broomva/lago.git ../lago && \
    git clone --depth 1 https://github.com/broomva/praxis.git ../praxis && \
    git clone --depth 1 https://github.com/broomva/autonomic.git ../autonomic && \
    git clone --depth 1 https://github.com/broomva/vigil.git ../vigil && \
    git clone --depth 1 https://github.com/broomva/haima.git ../haima && \
    git clone --depth 1 https://github.com/broomva/nous.git ../nous && \
    git clone --depth 1 https://github.com/broomva/anima.git ../anima
```

Also update the build-bust comment at line 3 to today's date.

- [ ] **Step 2: Commit**

```bash
git add Dockerfile
git commit -m "fix(docker): add CACHE_BUST ARG to prevent stale sibling repo clones

Docker layer caching can serve old git clones of aiOS/lago/etc even after
new commits are pushed. This caused build failures when arcan-sandbox
imported SubscriptionTier from aios-protocol before the aiOS clone had it.

The ARG invalidates the clone layer when Railway (or local builds) pass
a unique value."
```

---

## Task 3: Push, Verify CI, Deploy

- [ ] **Step 1: Push the branch**

```bash
git push origin main
```

- [ ] **Step 2: Monitor the Railway deployment**

```bash
railway deployment list --service arcan --limit 3 --json
```

Wait for status to change from `BUILDING` to `SUCCESS` (or `DEPLOYING`).

- [ ] **Step 3: Check runtime logs for healthy startup**

```bash
railway logs --service arcan --latest --lines 30 --json
```

Expected: `Starting arcan (remote Lago journal)` and `Listening` messages. No `hyper-util` panics.

- [ ] **Step 4: Verify health endpoint**

```bash
railway logs --service arcan --latest --lines 5 --filter "Listening" --json
```

The deployment is healthy when you see the `Listening` log line and no crash-loop.
