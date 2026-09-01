fn main() {
    let mut config = prost_build::Config::new();
    config.protoc_arg("--experimental_allow_proto3_optional");

    config.extern_path(".google.protobuf.Struct", "::prost_types::Struct");
    config.extern_path(".google.protobuf.Value", "::prost_types::Value");
    config.extern_path(".google.protobuf.ListValue", "::prost_types::ListValue");
    config.extern_path(".google.protobuf.NullValue", "::prost_types::NullValue");
    config.extern_path(".google.protobuf.Timestamp", "::prost_types::Timestamp");

    config
        .compile_protos(
            &[
                "confidence/events/v1/types.proto",
                "confidence/events/wasm/v1/wasm_api.proto",
            ],
            &["../openfeature-provider/proto"],
        )
        .unwrap_or_else(|e| panic!("Failed to compile protos {:?}", e));
}
