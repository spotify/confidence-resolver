fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new("../confidence-resolver/protos");
    let api = root.join("confidence/flags/resolver/v1/api.proto");
    println!("cargo:rerun-if-changed={}", root.display());
    tonic_build::configure()
        .compile_well_known_types(true)
        // The core owns the messages. Generate only transport bindings here.
        .extern_path(".confidence", "::confidence_resolver::proto::confidence")
        .extern_path(".google.protobuf", "::confidence_resolver::proto::google")
        .file_descriptor_set_path(
            std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("resolver_descriptor.bin"),
        )
        .compile_protos(&[api], &[root])?;
    // Remote materialization messages have a separate canonical provider schema.
    // Keep them in their own module so they cannot shadow core resolver messages.
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("remote");
    std::fs::create_dir_all(&out)?;
    let provider_root = std::path::Path::new("../openfeature-provider/proto");
    println!("cargo:rerun-if-changed={}", provider_root.display());
    prost_build::Config::new().out_dir(out).compile_protos(
        &[provider_root.join("confidence/flags/resolver/v1/internal_api.proto")],
        &[provider_root],
    )?;
    Ok(())
}
