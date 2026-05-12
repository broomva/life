//! End-to-end smoke test — the M7-W α completion gate.
//!
//! Spins up lifegw + lifed (and eventually arcan + lagod + haimad + animad)
//! via docker-compose. Submits a `createAgentTask` mutation via the patched
//! warp-oss binary (or via direct HTTP POST for CI). Subscribes to
//! `warpDriveUpdates` and waits for an `AmbientTaskUpdated` frame with
//! terminal status.
//!
//! Gated behind the `e2e-smoke` Cargo feature so it doesn't run in default
//! `cargo test`. CI runs it on a dedicated lane with Docker + Metal
//! Toolchain available.
//!
//! ## Operator pre-requisites (one-time)
//!
//! 1. macOS Metal Toolchain (only required when exercising the patched
//!    warp-oss client from a Mac — Linux CI doesn't need this):
//!    `xcodebuild -downloadComponent MetalToolchain`
//! 2. Build patched warp-oss (Phase β — currently the test drives the
//!    GraphQL endpoint via direct HTTP POST, matching what the Warp
//!    client does internally):
//!    `./apps/warp-oss-patch/apply-patch.sh`
//! 3. Build docker images:
//!    `docker compose -f tests/fixtures/docker-compose.e2e.yml build`
//!
//! ## Run
//!
//! ```bash
//! cargo test -p lifegw --features e2e-smoke \
//!     --test warp_graphql_e2e_smoke -- --test-threads=1
//! ```
//!
//! ## Phase α scope reality
//!
//! - The harness compiles under `--features e2e-smoke`.
//! - The docker-compose fixture currently uses the combined
//!   `lifegw-stack` image (`deploy/railway/lifegw-stack/Dockerfile`)
//!   which co-locates lifegw + lifed behind Caddy on port 8080.
//! - The WS subscription bridge (`ws_subscribe`) is stubbed with
//!   `todo!()` — the `graphql-ws-client` 0.10 API surface needs an
//!   integration pass against a live stack before un-stubbing. Run the
//!   test against a live stack first; the panic will guide
//!   implementation.
//! - `TestStack::up()` runs `docker compose up -d --build` and polls
//!   `/healthz` — that part is wire-complete.

#![cfg(feature = "e2e-smoke")]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::time::Duration;

use tokio::time::timeout;

const COMPOSE_FILE: &str = "tests/fixtures/docker-compose.e2e.yml";
/// Caddy-fronted public endpoint inside the `lifegw-stack` image. Caddy
/// listens on `:8080` cleartext (Railway-edge shape) and proxies into
/// `https://127.0.0.1:8443` where lifegw itself runs with a self-signed
/// cert. From a test driver's perspective the GraphQL endpoint is just
/// "8080 cleartext".
const ENDPOINT: &str = "http://127.0.0.1:8080/graphql/v2";
const WS_ENDPOINT: &str = "ws://127.0.0.1:8080/graphql-ws";
const HEALTHZ: &str = "http://127.0.0.1:8080/healthz";
const SMOKE_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::test]
async fn patched_warp_oss_client_drives_task_to_completion() {
    let _stack = TestStack::up().await;

    // Dev-mode token shortcut. `LIFED_ALLOW_MOCK_FALLBACK=true` is set
    // in the compose env so lifed accepts mocked substrate traffic;
    // lifegw's auth layer accepts `Bearer dev-token-for-{user_id}` when
    // built with the `dev-mode` feature (and the lifegw-stack image
    // ships with that feature for the Railway-style deployment).
    let token = "dev-token-for-test-user-1";

    // Submit createAgentTask via HTTP POST (proxies what the Warp
    // client would do internally).
    let create_resp = http_post_graphql(
        ENDPOINT,
        token,
        r#"mutation {
          createAgentTask(input: {
            prompt: "echo hello",
            modelId: "mock-default",
            environmentUid: "ws-test",
          }) {
            __typename
            ... on CreateAgentTaskOutput { id status }
          }
        }"#,
    )
    .await
    .expect("createAgentTask succeeds");

    let task_id = create_resp["data"]["createAgentTask"]["id"]
        .as_str()
        .expect("task id present")
        .to_string();

    // Subscribe to warpDriveUpdates and wait for terminal frame.
    let mut sub = ws_subscribe(WS_ENDPOINT, token)
        .await
        .expect("WS subscribe succeeds");

    let result = timeout(SMOKE_TIMEOUT, async {
        loop {
            let frame = sub.next_frame().await.expect("frame");
            if frame["__typename"] == "AmbientTaskUpdated"
                && frame["taskId"] == task_id
                && frame["status"] == "completed"
            {
                return;
            }
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "task did not reach completed within timeout"
    );
}

// --- Test infrastructure: TestStack + http_post_graphql + ws_subscribe ---

/// RAII stack manager — `docker compose up -d --build` on construction,
/// `docker compose down -v` on drop. Polls `/healthz` for readiness.
struct TestStack;

impl TestStack {
    async fn up() -> Self {
        let status = std::process::Command::new("docker")
            .args(["compose", "-f", COMPOSE_FILE, "up", "-d", "--build"])
            .status()
            .expect("docker compose up succeeds");
        assert!(status.success(), "docker compose up failed");

        // Wait for healthz — 60 attempts × 1s = 60s headroom.
        for _ in 0..60 {
            if let Ok(resp) = reqwest::get(HEALTHZ).await
                && resp.status().is_success()
            {
                return Self;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!("lifegw did not become healthy within 60s");
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

/// Phase α stub — the real implementation uses the `graphql-ws-client`
/// crate (already a dev-dep at v0.10) to negotiate the
/// `graphql-transport-ws` subprotocol async-graphql emits, send
/// `connection_init` with the bearer token, and subscribe to
/// `warpDriveUpdates`.
///
/// API reference (un-stub guide):
/// - The 0.10 crate exposes a `Client` builder; pair with the
///   `tungstenite` feature for the raw WS transport.
/// - Subscriptions are typed as `cynic::Operation` or
///   `graphql_client::GraphQLQuery` — for the smoke test we can use the
///   `serde_json::Value` escape hatch via a hand-rolled
///   `GraphQLOperation` impl.
/// - Source on disk:
///   `~/.cargo/registry/src/index.crates.io-*/graphql-ws-client-0.10*/src/lib.rs`
///
/// Un-stub flow: replace `todo!()` in `next_frame` with a `next().await`
/// off the stream that the builder returns; the `next` value type is
/// `Result<serde_json::Value, _>` once the operation is typed as
/// `Value`-output.
#[allow(dead_code)]
struct WsSubscription {
    // Real WS connection state will live here once un-stubbed:
    // `Pin<Box<dyn Stream<Item = Result<serde_json::Value, _>> + Send>>`.
}

impl WsSubscription {
    async fn next_frame(&mut self) -> Option<serde_json::Value> {
        todo!("implement WS frame consumption via graphql-ws-client 0.10")
    }
}

async fn ws_subscribe(_url: &str, _token: &str) -> Result<WsSubscription, anyhow::Error> {
    // The graphql-ws-client crate (0.10) provides a `Client::build()`
    // builder that negotiates `graphql-transport-ws` and streams
    // `Result<T, Error>` items for a subscription. The exact builder
    // API is at:
    //   ~/.cargo/registry/src/.../graphql-ws-client-0.10*/src/lib.rs
    //
    // Phase α: stubbed `todo!()`. Un-stub when running the test for
    // the first time against a live stack. The
    // `graphql_ws_client::ClientBuilder` + `tokio_tungstenite::connect_async`
    // pattern is the entry point.
    todo!("implement WS connection + connection_init bearer auth + subscribe to warpDriveUpdates")
}
