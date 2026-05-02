//! Build script for `life-kernel-proto`.
//!
//! Compiles `kernel.proto` (now at `core/life/proto/life/kernel/v1/kernel.proto`
//! per M3) plus the legacy v0 service protos (still at
//! `crates/life-kernel/life-kernel-proto/proto/`) into their generated
//! modules. The `tonic-prost-build` pipeline emits both server traits
//! and client stubs for each service.
//!
//! M3 (BRO-928): `kernel.proto` was migrated to the canonical
//! `core/life/proto/life/kernel/v1/` tree and now `import`s
//! `aios/v1/identifiers.proto` for shared opaque identifier types. The
//! generated code lives under `life.kernel.v1` (post-rename); the legacy
//! `broomva.life.kernel.v1` package is preserved as a deprecated Rust
//! alias in `lib.rs` for one minor version.
//!
//! M3.5 / M4: the eight v0 service protos (`approvals`, `events`,
//! `model`, `policy`, `relay`, `session`, `tools`, `common`) still live
//! at the legacy `proto/` path next to this crate. They migrate to
//! `core/life/proto/life/v1/` and `core/life/proto/life/admin/v1/` once
//! the lifed (facade) design dictates which dialect each belongs to.
//!
//! ## Transport choice (tonic over ttrpc)
//!
//! Spec A Phase 1 originally called for `ttrpc-rust` + `ttrpc-codegen`
//! to mirror Kata's control-plane wire format. In the `prost = "0.14"`
//! ecosystem `ttrpc-codegen 0.6` pulls in `rust-protobuf` 3.7 for its
//! message types and does not emit prost-compatible service stubs —
//! mixing its output with `prost-build`-generated messages is not
//! feasible without a custom fork. The workspace already pins
//! `tonic = "0.14"` / `tonic-prost = "0.14"` / `tonic-prost-build = "0.14"`
//! (used by `lago-ingest`), and tonic works equally well over a Unix
//! domain socket transport when the daemon lands in Phase 2. The
//! ttrpc-rust ↔ tonic deviation is documented on the BRO-857 commit
//! and will be reconciled if a concrete interop need surfaces.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // core/life/ root, three levels up from crates/life-kernel/life-kernel-proto/.
    let life_root = manifest_dir
        .parent() // crates/life-kernel/
        .and_then(|p| p.parent()) // crates/
        .and_then(|p| p.parent()) // core/life/
        .ok_or("walking up to core/life/")?
        .to_path_buf();

    // Canonical proto tree (M3): hosts kernel.proto + the aios.v1.* vocabulary.
    let canonical_proto_root = life_root.join("proto");
    let kernel_proto = canonical_proto_root.join("life/kernel/v1/kernel.proto");
    // Spec D D-Sub-E: soma admin custody-oracle service. Sibling of
    // `life.kernel.v1.KernelService` — mounted on the same admin UDS
    // (SO_PEERCRED + life-runtime group), but in a separate proto
    // package so per-RPC RBAC stays obvious.
    let custody_proto = canonical_proto_root.join("life/admin/kernel/v1/custody.proto");

    // Legacy v0 service protos still live alongside this crate's manifest
    // until M3.5 / M4 migrates them to proto/life/v1/ + proto/life/admin/v1/.
    let legacy_proto_root = manifest_dir.join("proto");
    let mut legacy_protos: Vec<std::path::PathBuf> = std::fs::read_dir(&legacy_proto_root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("proto"))
        .collect();
    legacy_protos.sort();

    let mut all_protos = vec![kernel_proto, custody_proto];
    all_protos.extend(legacy_protos);

    for p in &all_protos {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    // Pick up edits to imported aios/v1/*.proto files too — they're
    // compiled into the aios-proto crate but kernel.proto imports their
    // type names so a change there changes our generated output too.
    println!(
        "cargo:rerun-if-changed={}",
        canonical_proto_root
            .join("aios/v1/identifiers.proto")
            .display()
    );

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        // `extern_path` makes prost-build emit references to the canonical
        // `aios.v1.*` types as `::aios_proto::aios::v1::*` (the path
        // re-exported by the `aios-proto` crate) instead of redefining
        // them locally. This is what wires Layer 2's "import-don't-redefine"
        // contract into the generated Rust code.
        .extern_path(".aios.v1", "::aios_proto::aios::v1")
        .compile_protos(&all_protos, &[canonical_proto_root, legacy_proto_root])?;

    Ok(())
}
