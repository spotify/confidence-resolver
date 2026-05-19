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
        .executable(
            name: "LocalResolverCli",
            targets: ["LocalResolverCli"]
        ),
    ],
    dependencies: [
        .package(url: "https://github.com/swiftwasm/WasmKit.git", from: "0.1.7"),
        .package(url: "https://github.com/apple/swift-protobuf.git", from: "1.27.0"),
    ],
    targets: [
        .target(
            name: "ConfidenceLocalResolver",
            dependencies: [
                .product(name: "WasmKit", package: "WasmKit"),
                .product(name: "SwiftProtobuf", package: "swift-protobuf"),
            ],
            resources: [
                .copy("Resources/confidence_resolver.wasm"),
            ]
        ),
        .executableTarget(
            name: "LocalResolverCli",
            dependencies: ["ConfidenceLocalResolver"]
        ),
    ]
)
