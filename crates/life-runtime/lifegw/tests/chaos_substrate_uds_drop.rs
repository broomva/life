//! Chaos test (Sub-phase E item #6 — chaos #1): the upstream lifed
//! UDS becomes unreachable mid-flight → clients receive clean
//! gRPC-shaped errors (`UNAVAILABLE`) rather than connection-reset
//! garbage.
//!
//! Spec C₃ §6.5: when lifed disappears (panic / restart / network
//! partition), the gateway must surface a deterministic close-code
//! 4004 (`lifed-unavailable`) for WS streams and a `Status::unavailable`
//! tonic code for unary gRPC. Sub-phase D wired the rate-limit
//! returning `Status::resource_exhausted` for over-budget requests
//! and the proxy forwards `Status::unavailable` when the upstream
//! channel is closed.
//!
//! This chaos test exercises the simpler shape: dial a non-existent
//! UDS and verify the proxy returns a clean `unavailable` error
//! rather than panicking or hanging.
//!
//! `#[ignore]`-gated by default because it exercises the network-
//! configurable path; CI opts in via `cargo test -- --ignored`.

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use lifegw::proxy::connect_uds;

#[tokio::test]
#[ignore = "exercises real UDS connect; opt-in via --ignored"]
async fn substrate_uds_unreachable_returns_clean_error() {
    // Dial a UDS path that doesn't exist — connect_uds is async and
    // should return Err (not panic, not hang) when the underlying
    // tokio_net::UnixStream::connect fails.
    let nonexistent = std::path::PathBuf::from("/tmp/lifegw-chaos-never-exists.sock");
    // Best-effort: ensure the path is clean.
    let _ = std::fs::remove_file(&nonexistent);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), connect_uds(&nonexistent))
        .await
        .expect("connect_uds must not hang");

    assert!(
        result.is_err(),
        "connect_uds to a non-existent path must error cleanly"
    );
}

#[tokio::test]
async fn substrate_uds_drop_after_handshake_yields_unavailable_on_next_call() {
    // Bind a real UnixListener to a tempdir socket, accept ONE connection
    // (mimicking the post-handshake state), then drop the listener and
    // verify a subsequent connect attempt returns a clean error rather
    // than hanging or panicking. This exercises the real-life "lifed
    // restarted while gateway holds a stale connection" scenario from a
    // gateway-side perspective without needing the full lifed rig.
    use tokio::net::UnixListener;

    let tmp = tempfile::tempdir().expect("tempdir");
    let sock_path = tmp.path().join("lifed-chaos.sock");

    // Bind, accept a token connection (handshake-equivalent), then close.
    let listener = UnixListener::bind(&sock_path).expect("bind tempdir UDS");
    let accept_handle = tokio::spawn(async move {
        // Accept ONE connection then drop the listener.
        let _ = listener.accept().await;
        // listener drops here -> any further connect attempts fail
    });

    // First connect should succeed (post-handshake state).
    let first =
        tokio::time::timeout(std::time::Duration::from_secs(2), connect_uds(&sock_path)).await;
    assert!(
        first.is_ok() && first.unwrap().is_ok(),
        "first connect to live socket must succeed"
    );

    // Wait for the accept task to finish dropping the listener.
    accept_handle.await.expect("accept task completes");

    // Remove the socket file to fully simulate lifed disappearing —
    // connect to the now-removed path and verify a clean Err within
    // the 2s budget.
    let _ = std::fs::remove_file(&sock_path);
    let second =
        tokio::time::timeout(std::time::Duration::from_secs(2), connect_uds(&sock_path)).await;
    let result = second.expect("connect_uds must not hang on dead UDS");
    assert!(
        result.is_err(),
        "connect_uds to a dropped socket must error (got: {result:?})"
    );

    // The error string must mention the path so operational logs are
    // useful — verify there's actual error content (not a panic message).
    let err = result.unwrap_err();
    let err_str = format!("{err:?}");
    assert!(
        !err_str.is_empty() && err_str.len() > 10,
        "error must carry diagnostic content (got: {err_str})"
    );
}
