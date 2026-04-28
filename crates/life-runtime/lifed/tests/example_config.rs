//! Verifies that `LifedConfig::load(None)` produces a default config that
//! matches the expected master-spec defaults (UDS paths, drain budget, etc.).

use lifed::config::LifedConfig;

#[test]
fn default_config_matches_spec_defaults() {
    let cfg = LifedConfig::load(None).expect("default config loads");
    assert_eq!(
        cfg.public_plane.unix_socket.to_str(),
        Some("/run/life/life.sock")
    );
    assert_eq!(cfg.public_plane.unix_socket_mode, Some(0o660));
    assert_eq!(
        cfg.public_plane.unix_socket_group.as_deref(),
        Some("life-runtime")
    );
    assert_eq!(
        cfg.admin_plane.unix_socket.to_str(),
        Some("/run/life/life-admin.sock")
    );
    assert_eq!(cfg.admin_plane.unix_socket_mode, Some(0o660));
    assert_eq!(
        cfg.admin_plane.unix_socket_group.as_deref(),
        Some("life-admin")
    );
    assert_eq!(cfg.shutdown.drain_secs, 30);
    assert_eq!(cfg.routing.idle_threshold_secs, 3600);
    assert_eq!(cfg.routing.hard_cap, 100_000);
    assert_eq!(
        cfg.substrates.arcan.unix_socket.to_str(),
        Some("/run/life/arcan.sock")
    );
    assert_eq!(
        cfg.substrates.lago.unix_socket.to_str(),
        Some("/run/life/lago.sock")
    );
    assert_eq!(
        cfg.substrates.haima.unix_socket.to_str(),
        Some("/run/life/haima.sock")
    );
    assert_eq!(
        cfg.substrates.anima.unix_socket.to_str(),
        Some("/run/life/anima.sock")
    );
}

#[test]
fn parse_minimal_toml() {
    let toml_text = r#"
        [public_plane]
        unix_socket = "/tmp/lifed-test/life.sock"
    "#;
    let cfg: LifedConfig = toml::from_str(toml_text).expect("parses");
    assert_eq!(
        cfg.public_plane.unix_socket.to_str(),
        Some("/tmp/lifed-test/life.sock")
    );
    // Other fields defaulted.
    assert_eq!(
        cfg.admin_plane.unix_socket.to_str(),
        Some("/run/life/life-admin.sock")
    );
}

/// Sub-phase D follow-up #8: `run_daemon` refuses to start when a
/// substrate UDS is missing UNLESS the caller passes
/// `allow_mock_fallback = true`. This test writes a config pointing
/// each substrate at a guaranteed-missing path inside a tempdir and
/// confirms run_daemon fails with `LifedError::Substrate`.
#[tokio::test]
async fn run_daemon_refuses_mock_fallback_by_default() {
    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let cfg_path = tempdir.path().join("lifed.toml");
    let public_path = tempdir.path().join("public.sock");
    let admin_path = tempdir.path().join("admin.sock");
    let body = format!(
        r#"
[public_plane]
unix_socket = "{}"

[admin_plane]
unix_socket = "{}"

[substrates.arcan]
unix_socket = "{}/arcan.sock"

[substrates.lago]
unix_socket = "{}/lago.sock"

[substrates.haima]
unix_socket = "{}/haima.sock"

[substrates.anima]
unix_socket = "{}/anima.sock"

[substrates.soma]
unix_socket = "{}/soma.sock"
"#,
        public_path.display(),
        admin_path.display(),
        tempdir.path().display(),
        tempdir.path().display(),
        tempdir.path().display(),
        tempdir.path().display(),
        tempdir.path().display(),
    );
    std::fs::write(&cfg_path, body).expect("write config");

    let err = lifed::bootstrap::run_daemon(Some(&cfg_path), false)
        .await
        .expect_err("must refuse mock fallback by default");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("substrate UDS socket(s) missing"),
        "expected substrate-missing error, got {msg}",
    );
    assert!(
        msg.contains("--allow-mock-fallback"),
        "error must mention the opt-in flag, got {msg}",
    );
}
