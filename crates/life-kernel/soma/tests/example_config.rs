//! Guard against schema drift between `lifed.example.toml` and `SomaConfig`.
//!
//! If the example config file fails to parse, the schema has drifted from the
//! documented defaults — this test acts as the canary.

#[test]
fn example_toml_parses_into_lifed_config() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("deploy/config/lifed.example.toml");
    let cfg = lifed::SomaConfig::load(Some(&path)).expect("example config parses");
    assert!(cfg.backends.local);
    assert_eq!(cfg.lago.namespace, "lifed");
}
