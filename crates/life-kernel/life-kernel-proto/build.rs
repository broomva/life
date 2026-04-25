//! Build script for `life-kernel-proto`.
//!
//! Compiles every `.proto` file under `proto/` into its own generated
//! module. The `tonic-prost-build` pipeline emits both server traits
//! and client stubs for each service.
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
    let proto_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("proto");

    let proto_files: Vec<std::path::PathBuf> = std::fs::read_dir(&proto_root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("proto"))
        .collect();

    for proto in &proto_files {
        println!("cargo:rerun-if-changed={}", proto.display());
    }

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&proto_files, &[proto_root])?;

    Ok(())
}
