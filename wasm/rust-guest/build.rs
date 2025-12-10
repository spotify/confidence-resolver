fn main() {
    let mut config = prost_build::Config::new();
    // Needed for proto3 optional fields on older protoc versions available in CI images
    config.protoc_arg("--experimental_allow_proto3_optional");

    config.extern_path(
        ".google.protobuf.Struct",
        "::confidence_resolver::proto::google::Struct",
    );
    config.extern_path(
        ".google.protobuf.Value",
        "::confidence_resolver::proto::google::Value",
    );
    config.extern_path(
        ".google.protobuf.ListValue",
        "::confidence_resolver::proto::google::ListValue",
    );
    config.extern_path(
        ".google.protobuf.NullValue",
        "::confidence_resolver::proto::google::NullValue",
    );
    config.extern_path(
        ".google.protobuf.Timestamp",
        "::confidence_resolver::proto::google::Timestamp",
    );
    config.extern_path(
        ".confidence.shared.v1",
        "::confidence_resolver::proto::confidence::shared::v1",
    );

    config
        .compile_protos(&["messages.proto", "types.proto"], &["../proto"])
        .unwrap_or_else(|e| panic!("Failed to compile protos {:?}", e));
}
