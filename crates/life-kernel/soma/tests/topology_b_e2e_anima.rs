//! Topology-B end-to-end wire test for BRO-1019.
//!
//! Boots a minimal `IdentityState`, exposes `anima.v1.IdentitySubstrate`
//! on a tempdir UDS bound through the real `admin::listener::bind`
//! (so AdminConn → AdminConnInfo extension wiring matches production),
//! dials it via `anima-proxy`'s `AnimaProxy` builder, and asserts that:
//!
//! 1. `AnimaProxy::register_session(sid, user_id)` actually causes the
//!    underlying `IdentityState` to gain that session — proving the
//!    wire moves args through the substrate (not a hardcoded shape).
//! 2. `AnimaProxy::update_profile(...)` actually mutates the real
//!    substrate account, proving the wire transports the operation
//!    end-to-end.
//! 3. Proxy → server round-trip: register → get_account → list_sessions
//!    → revoke composes against the same in-memory state, with each
//!    call visible to the next.
//!
//! This is the contract the four-PR Topology-B audit (entity page
//! `research/entities/concept/topology-b-substrate-stub-gap.md`)
//! demanded a real wire for. Lifed isn't wired in here — adding it
//! would pull soma into the `lifed`/`anima-proxy` dep tree and break
//! `scripts/verify_dependencies_lifed.sh`. The lifed → anima-proxy
//! boundary is already covered by lifed's own integration suite (it
//! exercises the `AnimaCall` trait), so end-to-end coverage in
//! production is the COMPOSITION of those two suites — same pattern
//! as BRO-1016 / BRO-1017 / BRO-1018's `topology_b_e2e_*.rs`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anima_proxy::{AnimaProxy, Profile};
use anima_substrate_proto::anima::v1::identity_substrate_server::IdentitySubstrateServer;
use soma::admin::policy::AdminPolicy;
use soma::config::AdminPlaneConfig;
use soma::identity::{IdentityState, IdentitySubstrateService};
use tempfile::TempDir;
use tokio::sync::oneshot;

/// Spin up the substrate gRPC server on a tempdir UDS socket and
/// return the socket path + shutdown handle. The server consumes a
/// shared `Arc<IdentityState>` so the test can read its state after
/// driving calls through the proxy.
///
/// Uses `admin::listener::bind` so the per-connection AdminConn wraps
/// the stream with peer-cred info — matches production wiring rather
/// than bypassing it via a raw `UnixListenerStream`.
struct SubstrateUnderTest {
    socket: PathBuf,
    _tempdir: TempDir,
    state: Arc<IdentityState>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl SubstrateUnderTest {
    async fn start() -> Self {
        let tempdir = TempDir::new().expect("tempdir");
        let socket = tempdir.path().join("soma-admin.sock");
        let state = Arc::new(IdentityState::new());
        // Permissive policy — the listener still attaches PeerCred via
        // AdminConn, but every peer is admitted. Strict policy would
        // require the test process to belong to a specific group,
        // which CI can't guarantee.
        let policy = Arc::new(AdminPolicy::permissive());
        let service = IdentitySubstrateService::new(Arc::clone(&state), policy);

        // Use `unix_socket_group = None` so the listener does not try
        // to chown to a system group (CI runs as a non-root user with
        // no `life-runtime` group). Mode is also None to skip the
        // chmod path. `AdminPlaneConfig` is `#[non_exhaustive]` so we
        // mutate fields on a `Default::default()` instance.
        let mut admin_cfg = AdminPlaneConfig::default();
        admin_cfg.unix_socket = socket.clone();
        admin_cfg.unix_socket_mode = None;
        admin_cfg.unix_socket_group = None;

        let acceptor = soma::admin::listener::bind(&admin_cfg)
            .await
            .expect("bind admin UDS");

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let server_handle = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(IdentitySubstrateServer::new(service))
                .serve_with_incoming_shutdown(acceptor, async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        // Wait for the socket to appear.
        for _ in 0..200 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(socket.exists(), "substrate socket bound");

        Self {
            socket,
            _tempdir: tempdir,
            state,
            shutdown_tx: Some(shutdown_tx),
            server_handle: Some(server_handle),
        }
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.server_handle.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), h).await;
        }
    }
}

#[tokio::test]
async fn register_session_creates_real_session_record() {
    let env = SubstrateUnderTest::start().await;
    let proxy = AnimaProxy::connect(env.socket.clone())
        .await
        .expect("dial substrate UDS");

    proxy
        .register_session("bro-1019-reg-sid", "alice")
        .await
        .expect("register_session");

    // Substrate-side proof: BEFORE BRO-1019 this would have stayed at
    // 0 sessions because the proxy discarded its args and returned
    // Ok(()) without touching the substrate. AFTER BRO-1019 the
    // substrate IdentityState now has the session.
    assert_eq!(
        env.state.session_count(),
        1,
        "session materialised substrate-side"
    );
    let s = env
        .state
        .session("bro-1019-reg-sid")
        .expect("session present");
    assert_eq!(s.user_id, "alice");
    assert!(s.closed_at.is_none(), "session is open");

    // Idempotency: re-registering the same sid does NOT create a
    // duplicate.
    proxy
        .register_session("bro-1019-reg-sid", "alice")
        .await
        .expect("re-register idempotent");
    assert_eq!(env.state.session_count(), 1, "still one session");

    // mark_session_closed flips closed_at substrate-side.
    proxy
        .mark_session_closed("bro-1019-reg-sid")
        .await
        .expect("mark_session_closed");
    let after_close = env
        .state
        .session("bro-1019-reg-sid")
        .expect("still present");
    assert!(after_close.closed_at.is_some(), "closed_at set");

    // Idempotent: unknown sid is fine.
    proxy
        .mark_session_closed("sid-never-existed")
        .await
        .expect("idempotent close unknown");

    env.shutdown().await;
}

#[tokio::test]
async fn update_profile_returns_real_updated_account() {
    let env = SubstrateUnderTest::start().await;
    let proxy = AnimaProxy::connect(env.socket.clone())
        .await
        .expect("dial substrate UDS");

    // get_account materialises a default account substrate-side.
    let initial = proxy.get_account("bob").await.expect("get_account");
    assert_eq!(initial.user_id, "bob");
    assert_eq!(initial.handle, "@bob");
    assert_eq!(initial.tier, "free");
    assert_eq!(initial.profile.bio, "");

    // update_profile mutates the substrate.
    let mut prefs = std::collections::HashMap::new();
    prefs.insert("theme".to_string(), "dark".to_string());
    let new_profile = Profile {
        bio: "BRO-1019 e2e test".into(),
        avatar_blob_ref: vec![1, 2, 3, 4],
        preferences: prefs,
    };
    let updated = proxy
        .update_profile("bob", new_profile.clone())
        .await
        .expect("update_profile");

    // Substrate-side proof: BEFORE BRO-1019 this would have rebuilt
    // a hardcoded Account locally; AFTER BRO-1019 the returned shape
    // is derived from the server-side IdentityState.
    assert_eq!(updated.user_id, "bob");
    assert_eq!(updated.profile.bio, "BRO-1019 e2e test");
    assert_eq!(updated.profile.avatar_blob_ref, vec![1, 2, 3, 4]);
    assert_eq!(
        updated.profile.preferences.get("theme"),
        Some(&"dark".to_string())
    );

    // A subsequent get_account through the proxy returns the same
    // updated profile — the substrate is the single source of truth.
    let probe = proxy.get_account("bob").await.expect("re-get_account");
    assert_eq!(probe.profile.bio, "BRO-1019 e2e test");

    env.shutdown().await;
}

#[tokio::test]
async fn proxy_to_server_round_trip() {
    let env = SubstrateUnderTest::start().await;
    let proxy = AnimaProxy::connect(env.socket.clone())
        .await
        .expect("dial substrate UDS");

    // Compose the typical lifed Identity-handler path: register two
    // sessions for alice + one for bob, list them, revoke one, list
    // again.
    proxy
        .register_session("sid-rt-a", "alice")
        .await
        .expect("register a");
    // Force a beat so opened_at differs between sessions; list_sessions
    // sorts by opened_at and the test asserts ordering.
    tokio::time::sleep(Duration::from_millis(2)).await;
    proxy
        .register_session("sid-rt-b", "alice")
        .await
        .expect("register b");
    proxy
        .register_session("sid-rt-c", "bob")
        .await
        .expect("register c");

    // List alice's open sessions.
    let alice_open = proxy
        .list_sessions("alice", false, 0)
        .await
        .expect("list_sessions alice");
    assert_eq!(alice_open.len(), 2);
    assert_eq!(alice_open[0].sid, "sid-rt-a");
    assert_eq!(alice_open[1].sid, "sid-rt-b");
    assert_eq!(alice_open[0].closed_at_ms, 0, "session a is open");

    // Revoke sid-rt-a — should mark it closed substrate-side.
    proxy.revoke_session("sid-rt-a").await.expect("revoke a");
    let s = env.state.session("sid-rt-a").expect("present");
    assert!(s.closed_at.is_some(), "closed by revoke");
    assert!(s.revoked_at.is_some(), "revoked_at set");

    // Without include_closed, alice has only b.
    let alice_open2 = proxy
        .list_sessions("alice", false, 0)
        .await
        .expect("list alice 2");
    assert_eq!(alice_open2.len(), 1);
    assert_eq!(alice_open2[0].sid, "sid-rt-b");

    // With include_closed, alice has both — and a's closed_at_ms is
    // non-zero through the wire.
    let alice_all = proxy
        .list_sessions("alice", true, 0)
        .await
        .expect("list alice all");
    assert_eq!(alice_all.len(), 2);
    let a = alice_all.iter().find(|s| s.sid == "sid-rt-a").expect("a");
    assert!(a.closed_at_ms > 0, "closed_at_ms non-zero through wire");

    // bob has only c, not alice's sessions — proves user_id filtering
    // works on the substrate side.
    let bob_sessions = proxy
        .list_sessions("bob", true, 0)
        .await
        .expect("list bob");
    assert_eq!(bob_sessions.len(), 1);
    assert_eq!(bob_sessions[0].sid, "sid-rt-c");

    // get_account on a user with no profile change returns the default
    // shape, but `created_at_ms` is a real timestamp (not 0).
    let acc = proxy.get_account("alice").await.expect("get alice");
    assert_eq!(acc.user_id, "alice");
    assert!(acc.created_at_ms > 0);

    env.shutdown().await;
}
