use std::io::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Crate-local proto/ is used by cargo publish (tarball verification).
    // Shared workspace ../proto/ is the canonical source during development.
    let proto_root = {
        let local = manifest_dir.join("proto");
        if local.exists() {
            local
        } else {
            manifest_dir.join("..").join("proto")
        }
    };

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
