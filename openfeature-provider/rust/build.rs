use std::io::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    let proto_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("proto");

    let proto_files = vec![
        proto_root.join("confidence/flags/resolver/v1/types.proto"),
        proto_root.join("confidence/flags/resolver/v1/internal_api.proto"),
    ];

    for proto_file in &proto_files {
        println!("cargo:rerun-if-changed={}", proto_file.display());
    }

    prost_build::Config::new().compile_protos(&proto_files, &[&proto_root])?;

    Ok(())
}
