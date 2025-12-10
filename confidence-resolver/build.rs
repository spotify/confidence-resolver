use std::env;
use std::io::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    // Suppress all clippy lints in generated proto code
    const ALLOW_ATTR: &str = "#[allow(clippy::all, clippy::arithmetic_side_effects, clippy::panic, clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]";

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protos");
    let proto_files = vec![
        root.join("confidence/flags/admin/v1/types.proto"),
        root.join("confidence/flags/admin/v1/resolver.proto"),
        root.join("confidence/flags/resolver/v1/api.proto"),
        root.join("confidence/flags/resolver/v1/internal_api.proto"),
        root.join("confidence/flags/resolver/v1/wasm_api.proto"),
        root.join("confidence/flags/resolver/v1/events/events.proto"),
    ];

    // Tell cargo to recompile if any of these proto files are changed
    for proto_file in &proto_files {
        println!("cargo:rerun-if-changed={}", proto_file.display());
    }

    let descriptor_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("proto_descriptor.bin");

    let mut config = prost_build::Config::new();

    config.type_attribute(".", ALLOW_ATTR);

    [
        "confidence.flags.admin.v1.ClientResolveInfo.EvaluationContextSchemaInstance",
        "confidence.flags.admin.v1.ContextFieldSemanticType",
        "confidence.flags.admin.v1.ContextFieldSemanticType.type",
        "confidence.flags.admin.v1.ContextFieldSemanticType.VersionSemanticType",
        "confidence.flags.admin.v1.ContextFieldSemanticType.CountrySemanticType",
        "confidence.flags.admin.v1.ContextFieldSemanticType.TimestampSemanticType",
        "confidence.flags.admin.v1.ContextFieldSemanticType.DateSemanticType",
        "confidence.flags.admin.v1.ContextFieldSemanticType.EntitySemanticType",
        "confidence.flags.admin.v1.ContextFieldSemanticType.EnumSemanticType",
        "confidence.flags.admin.v1.ContextFieldSemanticType.EnumSemanticType.EnumValue",
    ]
    .iter()
    .for_each(|&p| {
        config.type_attribute(p, "#[derive(Eq, Hash)]");
    });

    config
        .file_descriptor_set_path(&descriptor_path)
        .btree_map(["."]);

    #[cfg(feature = "json")]
    {
        // Override prost-types with pbjson-types when std feature is enabled
        config
            .compile_well_known_types()
            .extern_path(".google.protobuf", "::pbjson_types");
    }

    // Generate prost structs
    config.compile_protos(&proto_files, &[root])?;

    #[cfg(feature = "json")]
    {
        // Generate pbjson serde implementations
        let descriptor_set = std::fs::read(&descriptor_path)?;
        pbjson_build::Builder::new()
            .register_descriptors(&descriptor_set)?
            .ignore_unknown_fields()
            .btree_map(["."])
            .build(&[
                ".confidence.flags.admin.v1",
                ".confidence.flags.resolver.v1",
                ".confidence.flags.resolver.v1.events",
                ".confidence.flags.types.v1",
                ".confidence.auth.v1",
                ".confidence.iam.v1",
                ".google.type",
            ])?;

        // Suppress all clippy lints in generated serde files
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        for entry in std::fs::read_dir(&out_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rs")
                && path
                    .file_name()
                    .is_some_and(|n| n.to_str().unwrap().contains(".serde.rs"))
            {
                let content = std::fs::read_to_string(&path)?;
                let mut new_content = content
                    .replace("\nimpl ", &format!("\n{}\nimpl ", ALLOW_ATTR))
                    .replace("\nimpl<", &format!("\n{}\nimpl<", ALLOW_ATTR));

                // Handle first impl if it's at the start of file
                if new_content.starts_with("impl ") || new_content.starts_with("impl<") {
                    new_content = format!("{}\n{}", ALLOW_ATTR, new_content);
                }

                std::fs::write(&path, new_content)?;
            }
        }
    }

    Ok(())
}
