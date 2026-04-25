//! Integration test for the Unix-socket listener.
//!
//! Verifies that:
//! 1. The socket file is created at the configured path.
//! 2. The socket has the requested file-permission mode (0660).
//! 3. A Unix-socket client can connect after bind.
//! 4. A shutdown signal causes the server task to exit cleanly.

use std::sync::Arc;
use std::time::Duration;

use aios_protocol::{
    hypervisor::{ForkSpec, VmHandle, VmSnapshotHandle, VmSpec},
    kernel::{KernelContext, KernelResult},
    ports::KernelPort,
    tool::{ToolCall, ToolResult},
};

// ── PanicIfCalledKernel ───────────────────────────────────────────────────────
//
// Minimal stub that satisfies `KernelPort`. All methods panic if called — the
// integration test only exercises the transport layer (socket bind + shutdown),
// not end-to-end RPCs.

struct PanicIfCalledKernel;

#[async_trait::async_trait]
impl KernelPort for PanicIfCalledKernel {
    async fn create_vm(&self, _spec: VmSpec, _ctx: KernelContext) -> KernelResult<VmHandle> {
        panic!("PanicIfCalledKernel::create_vm called unexpectedly")
    }

    async fn dispatch(
        &self,
        _vm: &VmHandle,
        _call: ToolCall,
        _ctx: &KernelContext,
    ) -> KernelResult<ToolResult> {
        panic!("PanicIfCalledKernel::dispatch called unexpectedly")
    }

    async fn snapshot(&self, _vm: &VmHandle, _name: &str) -> KernelResult<VmSnapshotHandle> {
        panic!("PanicIfCalledKernel::snapshot called unexpectedly")
    }

    async fn fork(
        &self,
        _snapshot: &VmSnapshotHandle,
        _spec: ForkSpec,
        _ctx: KernelContext,
    ) -> KernelResult<VmHandle> {
        panic!("PanicIfCalledKernel::fork called unexpectedly")
    }

    async fn hibernate(&self, _vm: &VmHandle) -> KernelResult<()> {
        panic!("PanicIfCalledKernel::hibernate called unexpectedly")
    }

    async fn resume(&self, _vm: &VmHandle) -> KernelResult<VmHandle> {
        panic!("PanicIfCalledKernel::resume called unexpectedly")
    }

    async fn destroy(&self, _vm: VmHandle) -> KernelResult<()> {
        panic!("PanicIfCalledKernel::destroy called unexpectedly")
    }
}

// ── Test ──────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn listener_accepts_connection_and_shuts_down() {
    let tmpdir = tempfile::tempdir().unwrap();
    let socket = tmpdir.path().join("lifed.sock");

    // `SomaConfig` and `ServerConfig` are `#[non_exhaustive]`, so we cannot
    // construct them via struct literals outside the defining crate.
    // Use `Default::default()` and then mutate the public fields we care about.
    let mut cfg = lifed::SomaConfig::default();
    cfg.server.unix_socket = socket.clone();
    cfg.server.unix_socket_mode = Some(0o660);
    cfg.server.unix_socket_group = None;
    cfg.server.vsock = None;
    cfg.server.drain_secs = 5;

    let stub_engine: Arc<PanicIfCalledKernel> = Arc::new(PanicIfCalledKernel);

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server_cfg = cfg.clone();
    let server_task = tokio::spawn(async move {
        lifed::listener::serve(&server_cfg, stub_engine, shutdown_rx, Vec::new()).await
    });

    // Wait up to 2 s for the socket to appear and permissions to land.
    let mut tries = 0;
    while tries < 20 && !socket.exists() {
        tokio::time::sleep(Duration::from_millis(100)).await;
        tries += 1;
    }
    assert!(socket.exists(), "socket not created within 2 s");

    // Assert permissions are 0660.
    let meta = std::fs::metadata(&socket).unwrap();
    let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777;
    assert_eq!(
        mode, 0o660,
        "socket permissions are {mode:o}, expected 0660"
    );

    // Connect once as a basic sanity check that the listener is accepting.
    // Drop the stream immediately so tonic has no in-flight connections to
    // drain before honouring the shutdown signal.
    {
        let _stream = tokio::net::UnixStream::connect(&socket)
            .await
            .expect("client connect failed");
        // `_stream` drops here.
    }

    // Trigger shutdown and await termination.
    shutdown_tx.send(()).expect("shutdown_rx still alive");

    let serve_result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task did not shut down in time")
        .expect("server task panicked");

    assert!(
        serve_result.is_ok(),
        "serve returned an error: {serve_result:?}"
    );

    // Socket file should be gone after shutdown — tonic cleans up OR
    // `prepare_socket_path` handles it on the next bind. Either way, the
    // important invariant is that `serve` returned `Ok`.
}
