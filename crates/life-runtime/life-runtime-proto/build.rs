//! Codegen for `life.v1.*` (public plane) + `life.admin.v1.*` (admin plane).
//!
//! Source proto files live under `core/life/proto/life/v1/` and
//! `core/life/proto/life/admin/v1/`. Both subtrees import `aios.v1.*` from
//! `core/life/proto/aios/v1/` (M3-shipped canonical vocabulary).

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    // From crates/life-runtime/life-runtime-proto/, the proto root is ../../../proto/.
    let proto_root = manifest_dir
        .parent() // crates/life-runtime/
        .and_then(|p| p.parent()) // crates/
        .and_then(|p| p.parent()) // core/life/
        .ok_or("walking up to core/life/")?
        .join("proto");
    let life_v1 = proto_root.join("life/v1");
    let life_admin_v1 = proto_root.join("life/admin/v1");

    let inputs: Vec<PathBuf> = vec![
        life_v1.join("agent.proto"),
        life_v1.join("events.proto"),
        life_v1.join("wallet.proto"),
        life_v1.join("identity.proto"),
        life_admin_v1.join("runtime.proto"),
        life_admin_v1.join("saga.proto"),
        life_admin_v1.join("routing_cache.proto"),
    ];

    // Tell cargo to rerun on any proto change.
    for p in &inputs {
        println!("cargo:rerun-if-changed={}", p.display());
    }

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        // Reuse the canonical aios.v1 types from the aios-proto crate instead
        // of regenerating them inside this crate. This keeps a single Rust
        // representation per wire type (Spec C₂ §10.3) and avoids duplicate
        // symbol definitions when both crates are linked into the same binary.
        .extern_path(".aios.v1", "::aios_proto::aios::v1")
        .compile_protos(
            &inputs.iter().map(|p| p.as_path()).collect::<Vec<_>>(),
            &[proto_root.as_path()],
        )?;

    Ok(())
}
