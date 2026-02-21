use std::env;
use std::fmt::Write as _;
use std::io::Result;
use std::path::PathBuf;

use prost_reflect::{DescriptorPool, Value};

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

    generate_telemetry_config(&descriptor_path)?;

    Ok(())
}

/// Scan proto descriptors for messages annotated with `(confidence.api.histogram)`
/// and generate `HistogramConfig` trait impls for the corresponding prost types.
fn generate_telemetry_config(descriptor_path: &std::path::Path) -> Result<()> {
    let descriptor_bytes = std::fs::read(descriptor_path)?;
    let pool = DescriptorPool::decode(descriptor_bytes.as_ref())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let histogram_ext = pool
        .get_extension_by_name("confidence.api.histogram")
        .expect("confidence.api.histogram extension not found in descriptor pool");

    let mut code = String::new();

    for msg in pool.all_messages() {
        let opts = msg.options();
        if !opts.has_extension(&histogram_ext) {
            continue;
        }
        let val = opts.get_extension(&histogram_ext);

        let histogram_msg = match val.as_ref() {
            Value::Message(m) => m,
            _ => continue,
        };

        let min_value = histogram_msg
            .get_field_by_name("min_value")
            .and_then(|v| v.as_i32())
            .unwrap_or(0);
        let max_value = histogram_msg
            .get_field_by_name("max_value")
            .and_then(|v| v.as_i32())
            .unwrap_or(0);
        let bucket_count = histogram_msg
            .get_field_by_name("bucket_count")
            .and_then(|v| v.as_i32())
            .unwrap_or(0);
        let unit = histogram_msg
            .get_field_by_name("unit")
            .and_then(|v| v.as_str().map(|s| s.to_owned()))
            .unwrap_or_default();

        let rust_type = proto_name_to_rust_type(msg.full_name());

        writeln!(
            code,
            "impl crate::telemetry::HistogramConfig for crate::proto::{rust_type} {{"
        )
        .unwrap();
        writeln!(code, "    const MIN_VALUE: u32 = {min_value};").unwrap();
        writeln!(code, "    const MAX_VALUE: u32 = {max_value};").unwrap();
        writeln!(code, "    const BUCKET_COUNT: usize = {bucket_count};").unwrap();
        writeln!(code, "    const UNIT: &'static str = \"{unit}\";").unwrap();
        writeln!(code, "}}").unwrap();
        writeln!(code).unwrap();
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    std::fs::write(out_dir.join("telemetry_config.rs"), code)?;

    Ok(())
}

/// Convert a fully-qualified proto name like
/// `confidence.flags.resolver.v1.TelemetryData.ResolveLatency`
/// into a Rust module path like
/// `confidence::flags::resolver::v1::telemetry_data::ResolveLatency`.
fn proto_name_to_rust_type(full_name: &str) -> String {
    let parts: Vec<&str> = full_name.split('.').collect();
    let mut result = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            result.push_str("::");
        }
        // In prost, parent message names become snake_case modules,
        // but the final type name stays PascalCase. We also need to handle
        // package segments (all lowercase) vs message names (PascalCase).
        if i < parts.len() - 1 && part.chars().next().map_or(false, |c| c.is_uppercase()) {
            // This is a parent message name — prost generates a module for it in snake_case
            result.push_str(&to_snake_case(part));
        } else {
            result.push_str(part);
        }
    }
    result
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(ch.to_ascii_lowercase());
    }
    result
}
