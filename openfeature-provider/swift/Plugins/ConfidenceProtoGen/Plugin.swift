// NOTE(unit-local experiment): Tiny build-tool plugin that generates the
// SwiftProtobuf bindings for ConfidenceLocalResolver from the shared proto
// directory at openfeature-provider/proto (symlinked in as `Protos/`). Uses
// `--swift_opt=FileNaming=PathToUnderscores` so the two `types.proto` files
// (resolver vs. types packages) don't collide on the same `.o` basename.
//
// We don't use the upstream SwiftProtobufPlugin because:
//   * it doesn't know how to declare output paths under PathToUnderscores
//     (it pre-computes nested paths matching the proto layout, while
//     protoc-gen-swift writes flat underscored filenames);
//   * without PathToUnderscores, the two `types.pb.swift` files collide
//     in SPM's per-target `.o` directory.
//
// Using `prebuildCommand` lets SPM auto-discover whatever protoc writes,
// which sidesteps both problems.
//
// Requirements:
//   * `protoc` must be on PATH or pointed at via the `PROTOC_PATH` env var.
//     The plugin falls back to `/opt/homebrew/bin/protoc` for Apple Silicon
//     macs (the developer audience for this experiment).

import Foundation
import PackagePlugin

@main
struct ConfidenceProtoGen: BuildToolPlugin {
    func createBuildCommands(context: PluginContext, target: Target) throws -> [Command] {
        guard let swiftTarget = target as? SwiftSourceModuleTarget else { return [] }

        let protoDir = swiftTarget.directory.appending(subpath: "Protos")
        let outputDir = context.pluginWorkDirectory.appending(subpath: "Generated")

        let protocPath = Self.resolveProtocPath()
        let protocGenSwift = try context.tool(named: "protoc-gen-swift").path

        let protos = [
            "confidence/wasm/messages.proto",
            "confidence/wasm/wasm_api.proto",
            "confidence/flags/resolver/v1/api.proto",
            "confidence/flags/resolver/v1/types.proto",
            "confidence/flags/types/v1/types.proto",
            "confidence/flags/types/v1/target.proto",
        ]

        // Output filenames under PathToUnderscores: drop ".proto", replace "/" with "_",
        // then append ".pb.swift".
        let outputFiles: [Path] = protos.map { proto -> Path in
            let stem = String(proto.dropLast(".proto".count))
            let underscored = stem.replacingOccurrences(of: "/", with: "_")
            return outputDir.appending(subpath: "\(underscored).pb.swift")
        }

        let inputFiles: [Path] = protos.map { protoDir.appending(subpath: $0) }

        var args: [String] = [
            "--proto_path=\(protoDir.string)",
            "--plugin=protoc-gen-swift=\(protocGenSwift.string)",
            "--swift_out=\(outputDir.string)",
            "--swift_opt=Visibility=Public",
            "--swift_opt=FileNaming=PathToUnderscores",
        ]
        args.append(contentsOf: protos)

        return [
            .buildCommand(
                displayName: "Generating Swift bindings from .proto files",
                executable: protocPath,
                arguments: args,
                inputFiles: inputFiles,
                outputFiles: outputFiles
            ),
        ]
    }

    private static func resolveProtocPath() -> Path {
        let env = ProcessInfo.processInfo.environment
        if let envPath = env["PROTOC_PATH"], !envPath.isEmpty {
            return Path(envPath)
        }
        let candidates = [
            "/opt/homebrew/bin/protoc",
            "/usr/local/bin/protoc",
            "/usr/bin/protoc",
        ]
        for candidate in candidates where FileManager.default.isExecutableFile(atPath: candidate) {
            return Path(candidate)
        }
        // Last-ditch: let the OS resolve via PATH (works in CLI builds where
        // /usr/bin/env passes PATH through).
        return Path("/usr/bin/env")
    }
}
