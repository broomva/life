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
