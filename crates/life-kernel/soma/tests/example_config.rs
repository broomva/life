//! Guard against schema drift between `soma.example.toml` and `SomaConfig`.
//!
//! If the example config file fails to parse, the schema has drifted from the
//! documented defaults — this test acts as the canary.

#[test]
fn example_toml_parses_into_soma_config() {
    // XXX(M0/Task 8): rename to soma.example.toml when deploy artifacts land.
    // The lifed.example.toml file still contains `namespace = "lifed"` so the
    // assertion below is intentionally pinned to that value until Task 8
    // renames the file AND flips both this path and the assertion.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("deploy/config/lifed.example.toml");
    let cfg = soma::SomaConfig::load(Some(&path)).expect("example config parses");
    assert!(cfg.backends.local);
    assert_eq!(cfg.lago.namespace, "lifed");
}
