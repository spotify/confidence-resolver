import ConfidenceLocalResolver
import Foundation
import SwiftProtobuf

@main
struct LocalResolverCli {
    static func main() async {
        do {
            try await run()
        } catch {
            FileHandle.standardError.write(Data("error: \(error)\n".utf8))
            exit(1)
        }
    }

    static func run() async throws {
        let arguments = CommandLine.arguments.dropFirst()
        let unit = arguments.first ?? "user_42"

        let serverURL = URL(string: ProcessInfo.processInfo.environment["UNIT_LOCAL_SERVER"] ?? "http://127.0.0.1:8787")!
        let clientSecret = ProcessInfo.processInfo.environment["CONFIDENCE_CLIENT_SECRET"] ?? "mkjJruAATQWjeY7foFIWfVAcBWnci2YF"

        print("[cli] fetching account config from \(serverURL)…")
        let client = SliceClient(baseURL: serverURL)
        let config = try await client.accountConfig()
        print("[cli] account_id=\(config.accountId) state_file_hash=\(config.stateFileHash) units=\(config.randomizationUnitFields) full_state=\(config.fullStateSizeBytes) bytes")

        print("[cli] fetching slice for unit=\(unit)…")
        let slice = try await client.fetchSlice(stateFileHash: config.stateFileHash, unit: unit)
        print("[cli] slice_bytes=\(slice.bytes.count)  ratio=\(String(format: "%.2f%%", Double(slice.bytes.count) / Double(config.fullStateSizeBytes) * 100))")

        print("[cli] initialising WASM resolver…")
        let resolver = try LocalResolver()
        try resolver.setResolverState(slice.bytes, accountId: config.accountId)

        var ctx = Google_Protobuf_Struct()
        for field in slice.randomizationUnitFields {
            ctx.fields[field] = Google_Protobuf_Value.with { $0.stringValue = unit }
        }
        ctx.fields["country"] = Google_Protobuf_Value.with { $0.stringValue = "SE" }

        print("[cli] resolving flags with evaluation_context: \(ctx.fields.mapValues { stringValueDebug($0) })")
        let response = try resolver.resolveFlags(clientSecret: clientSecret, evaluationContext: ctx, apply: true)
        print("[cli] resolved_flags=\(response.resolvedFlags.count) resolve_token_size=\(response.resolveToken.count)")
        for flag in response.resolvedFlags {
            let reason = String(describing: flag.reason).replacingOccurrences(of: "RESOLVE_REASON_", with: "")
            let variantSuffix = flag.variant.isEmpty ? "" : " variant=\(flag.variant)"
            let valueRepr = formatValue(flag.value)
            print("  - flag=\(flag.flag) reason=\(reason)\(variantSuffix) value=\(valueRepr)")
        }
    }

    private static func formatValue(_ value: Google_Protobuf_Struct) -> String {
        if value.fields.isEmpty { return "{}" }
        let inner = value.fields.map { "\($0.key): \(stringValueDebug($0.value))" }
            .sorted()
            .joined(separator: ", ")
        return "{ \(inner) }"
    }

    private static func stringValueDebug(_ value: Google_Protobuf_Value) -> String {
        switch value.kind {
        case .stringValue(let s): return "\"\(s)\""
        case .numberValue(let n): return "\(n)"
        case .boolValue(let b): return "\(b)"
        case .nullValue: return "null"
        case .listValue(let list): return "[" + list.values.map(stringValueDebug).joined(separator: ", ") + "]"
        case .structValue(let s): return formatValue(s)
        case nil: return "nil"
        }
    }
}
