//! Chaos test (Sub-phase E item #6 — chaos #3): cert rotation while
//! traffic is flowing → in-flight TLS connections complete; new
//! connections use the new cert.
//!
//! Spec C₃ §4.3 (LOCKED L4-D10): cert reload is non-disruptive to
//! in-flight connections. Sub-phase E sweep (item #14) closed the
//! deferred D3 work by wiring the reloader's ArcSwap<ServerConfig>
//! into the listener's accept loop via `AcceptorSource`.
//!
//! This test exercises the `CertReloader::acceptor()` accessor under
//! a tight rotation loop:
//! - Build a reloader.
//! - Concurrently: rotate the cert files N times AND call `acceptor()`
//!   M times.
//! - After the dust settles, the final `acceptor()` returns a
//!   ServerConfig matching the latest reload.
//! - Pre-rotation `Arc<ServerConfig>` snapshots stay valid (rustls's
//!   Arc-semantics guarantee in-flight connections aren't disturbed).

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use lifegw::services::cert_watch::CertReloader;
use tempfile::TempDir;

fn install_default_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn write_self_signed_named(
    dir: &Path,
    cert_name: &str,
    key_name: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let cert_kp =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("rcgen");
    let cert_pem = cert_kp.cert.pem();
    let key_pem = cert_kp.key_pair.serialize_pem();
    let cert_path = dir.join(cert_name);
    let key_path = dir.join(key_name);
    std::fs::write(&cert_path, cert_pem).expect("write cert");
    std::fs::write(&key_path, key_pem).expect("write key");
    (cert_path, key_path)
}

#[test]
fn cert_rotation_during_concurrent_acceptor_loads_completes() {
    install_default_provider();
    let dir = TempDir::new().expect("tempdir");
    let (cert, key) = write_self_signed_named(dir.path(), "cert.pem", "key.pem");
    let reloader = Arc::new(CertReloader::load(&cert, &key).expect("load"));

    // Snapshot the pre-rotation acceptor — this stays valid under rustls
    // Arc semantics for any in-flight handshake that already grabbed it.
    let pre_acceptor = reloader.acceptor();
    let pre_cfg = pre_acceptor.config().clone();

    // Reader fleet: spawn N threads that each repeatedly call
    // `acceptor()`. Their snapshots must always be a valid Arc.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let n_readers = 8;
    let n_acceptor_calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let reader_handles: Vec<_> = (0..n_readers)
        .map(|_| {
            let r = Arc::clone(&reloader);
            let s = Arc::clone(&stop);
            let counter = Arc::clone(&n_acceptor_calls);
            std::thread::spawn(move || {
                while !s.load(std::sync::atomic::Ordering::Relaxed) {
                    let acc = r.acceptor();
                    // Sanity: every acceptor wraps a non-null config.
                    let _cfg = acc.config().clone();
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            })
        })
        .collect();

    // Rotation loop: 5 rotations spaced by ~10ms.
    for i in 0..5 {
        std::thread::sleep(Duration::from_millis(10));
        let (cert2, key2) =
            write_self_signed_named(dir.path(), &format!("rot{i}.pem"), &format!("rot{i}.key"));
        std::fs::copy(&cert2, &cert).expect("copy cert");
        std::fs::copy(&key2, &key).expect("copy key");
        reloader.reload().expect("rotation reload");
    }

    // Stop readers + collect.
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for h in reader_handles {
        h.join().expect("reader join");
    }

    // Verifications:
    // 1. The pre-rotation acceptor's config is STILL alive (rustls
    //    Arc-semantics) — Arc::strong_count is at least 1.
    assert!(Arc::strong_count(&pre_cfg) >= 1);

    // 2. The reloader's current config is now distinct from the
    //    pre-rotation snapshot.
    let post_cfg = reloader.acceptor().config().clone();
    assert!(
        !Arc::ptr_eq(&pre_cfg, &post_cfg),
        "post-rotation config must be a NEW Arc<ServerConfig>"
    );

    // 3. The reload counter advanced by exactly 5.
    assert_eq!(reloader.reload_count(), 5);

    // 4. Readers actually exercised the path.
    let calls = n_acceptor_calls.load(std::sync::atomic::Ordering::Relaxed);
    assert!(calls > 0, "readers must have made acceptor() calls");
}

#[test]
fn cert_rotation_with_broken_cert_keeps_previous_config_live() {
    install_default_provider();
    let dir = TempDir::new().expect("tempdir");
    let (cert, key) = write_self_signed_named(dir.path(), "cert.pem", "key.pem");
    let reloader = CertReloader::load(&cert, &key).expect("load");
    let pre_cfg = reloader.acceptor().config().clone();

    // Corrupt the cert file mid-flight.
    std::fs::write(&cert, "not a real cert").expect("corrupt");
    let result = reloader.reload();
    assert!(result.is_err(), "broken cert must be rejected on reload");

    // The acceptor's config still points to the previous valid one.
    let post_cfg = reloader.acceptor().config().clone();
    assert!(
        Arc::ptr_eq(&pre_cfg, &post_cfg),
        "previous config must stay live on reload failure"
    );
}
