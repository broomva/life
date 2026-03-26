fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/a2a/v1/a2a.proto"], &["proto"])?;
    Ok(())
}
