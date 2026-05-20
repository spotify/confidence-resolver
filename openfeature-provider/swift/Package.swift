// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "ConfidenceLocalResolver",
    platforms: [
        .macOS(.v14),
        .iOS(.v16),
    ],
    products: [
        .library(
            name: "ConfidenceLocalResolver",
            targets: ["ConfidenceLocalResolver"]
        ),
        .library(
            name: "ConfidenceLocalProvider",
            targets: ["ConfidenceLocalProvider"]
        ),
        .executable(
            name: "LocalResolverCli",
            targets: ["LocalResolverCli"]
        ),
    ],
    dependencies: [
        .package(url: "https://github.com/swiftwasm/WasmKit.git", from: "0.1.7"),
        .package(url: "https://github.com/apple/swift-protobuf.git", from: "1.27.0"),
        .package(url: "https://github.com/open-feature/swift-sdk.git", .exact("0.5.0")),
    ],
    targets: [
        .target(
            name: "ConfidenceLocalResolver",
            dependencies: [
                .product(name: "WasmKit", package: "WasmKit"),
                .product(name: "SwiftProtobuf", package: "swift-protobuf"),
            ],
            exclude: ["Protos"],
            resources: [
                .copy("Resources/confidence_resolver.wasm"),
            ],
            plugins: [
                .plugin(name: "ConfidenceProtoGen"),
            ]
        ),
        .plugin(
            name: "ConfidenceProtoGen",
            capability: .buildTool(),
            dependencies: [
                .product(name: "protoc-gen-swift", package: "swift-protobuf"),
            ]
        ),
        .target(
            name: "ConfidenceLocalProvider",
            dependencies: [
                "ConfidenceLocalResolver",
                .product(name: "OpenFeature", package: "swift-sdk"),
                .product(name: "SwiftProtobuf", package: "swift-protobuf"),
            ]
        ),
        .executableTarget(
            name: "LocalResolverCli",
            dependencies: ["ConfidenceLocalResolver"]
        ),
    ]
)
