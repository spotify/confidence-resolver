import Foundation
import SwiftProtobuf
import WasmKit

/// A WASM-backed local Confidence resolver.
///
/// Loads the embedded `confidence_resolver.wasm` artifact, wires up the
/// `wasm_msg_host_*` imports, and exposes a small typed API for the unit-local
/// experiment: load a (sliced) `ResolverState` once, then run as many resolves
/// as needed against changing evaluation contexts.
///
/// Not thread-safe. Use one instance per caller, or serialise calls externally.
public final class LocalResolver {
    private let engine: Engine
    private let store: Store
    private let instance: Instance
    private let env: WasmEnv
    private let setResolverStateFn: Function
    private let resolveFlagsFn: Function
    private let applyFlagsFn: Function

    /// Loads the WASM module from the package bundle and instantiates it with the
    /// `log_message` and `current_time` host imports.
    public init() throws {
        guard let url = Bundle.module.url(forResource: "confidence_resolver", withExtension: "wasm") else {
            throw LocalResolverError.wasmResourceNotFound
        }
        let wasmBytes = try Data(contentsOf: url)
        let module = try parseWasm(bytes: Array(wasmBytes))

        let engine = Engine()
        let store = Store(engine: engine)
        let env = WasmEnv()

        let logMessageImport = Function(store: store, parameters: [.i32], results: [.i32]) { _, args in
            let requestPtr = args[0].i32
            do {
                let log = try WasmMsg.consumeRequest(Int32(bitPattern: requestPtr), as: Confidence_Flags_Resolver_V1_LogMessage.self, env: env)
                print("[wasm] \(log.message)")
                let respPtr = try WasmMsg.transferResponseSuccess(Confidence_Wasm_Void(), env: env)
                return [.i32(UInt32(bitPattern: respPtr))]
            } catch {
                let respPtr = (try? WasmMsg.transferResponseError(String(describing: error), env: env)) ?? 0
                return [.i32(UInt32(bitPattern: respPtr))]
            }
        }

        let currentTimeImport = Function(store: store, parameters: [.i32], results: [.i32]) { _, args in
            let requestPtr = args[0].i32
            do {
                _ = try WasmMsg.consumeRequest(Int32(bitPattern: requestPtr), as: Confidence_Wasm_Void.self, env: env)
                let now = Date()
                var ts = Google_Protobuf_Timestamp()
                let epoch = now.timeIntervalSince1970
                ts.seconds = Int64(epoch)
                ts.nanos = Int32((epoch - Double(Int64(epoch))) * 1_000_000_000)
                let respPtr = try WasmMsg.transferResponseSuccess(ts, env: env)
                return [.i32(UInt32(bitPattern: respPtr))]
            } catch {
                let respPtr = (try? WasmMsg.transferResponseError(String(describing: error), env: env)) ?? 0
                return [.i32(UInt32(bitPattern: respPtr))]
            }
        }

        let imports: Imports = [
            "wasm_msg": [
                "wasm_msg_host_log_message": logMessageImport,
                "wasm_msg_host_current_time": currentTimeImport,
            ]
        ]

        let instance = try module.instantiate(store: store, imports: imports)

        guard let memory = instance.exports[memory: "memory"] else {
            throw LocalResolverError.missingExport("memory")
        }
        guard let alloc = instance.exports[function: "wasm_msg_alloc"] else {
            throw LocalResolverError.missingExport("wasm_msg_alloc")
        }
        guard let free = instance.exports[function: "wasm_msg_free"] else {
            throw LocalResolverError.missingExport("wasm_msg_free")
        }
        guard let setResolverStateFn = instance.exports[function: "wasm_msg_guest_set_resolver_state"] else {
            throw LocalResolverError.missingExport("wasm_msg_guest_set_resolver_state")
        }
        guard let resolveFlagsFn = instance.exports[function: "wasm_msg_guest_resolve_flags"] else {
            throw LocalResolverError.missingExport("wasm_msg_guest_resolve_flags")
        }
        guard let applyFlagsFn = instance.exports[function: "wasm_msg_guest_apply_flags"] else {
            throw LocalResolverError.missingExport("wasm_msg_guest_apply_flags")
        }

        env.memory = memory
        env.alloc = alloc
        env.free = free

        self.engine = engine
        self.store = store
        self.instance = instance
        self.env = env
        self.setResolverStateFn = setResolverStateFn
        self.resolveFlagsFn = resolveFlagsFn
        self.applyFlagsFn = applyFlagsFn
    }

    /// Loads a (sliced) `ResolverState` into the WASM instance for the given account.
    ///
    /// `stateBytes` should be the protobuf-encoded `confidence.flags.admin.v1.ResolverState`
    /// served by the unit-local slice endpoint.
    public func setResolverState(_ stateBytes: Data, accountId: String) throws {
        var request = Confidence_Wasm_SetResolverStateRequest()
        request.state = stateBytes
        request.accountID = accountId
        let reqPtr = try WasmMsg.transferRequest(request, env: env)
        let result = try setResolverStateFn.invoke([.i32(UInt32(bitPattern: reqPtr))])
        let respPtr = Int32(bitPattern: result[0].i32)
        _ = try WasmMsg.consumeResponse(respPtr, as: Confidence_Wasm_Void.self, env: env)
    }

    /// Runs a resolve in `WithoutMaterializations` mode (which is appropriate for the
    /// unit-local experiment since materializations are baked into the slice).
    public func resolveFlags(
        clientSecret: String,
        evaluationContext: Google_Protobuf_Struct,
        flags: [String] = [],
        apply: Bool = false
    ) throws -> Confidence_Flags_Resolver_V1_ResolveFlagsResponse {
        var inner = Confidence_Flags_Resolver_V1_ResolveFlagsRequest()
        inner.clientSecret = clientSecret
        inner.evaluationContext = evaluationContext
        inner.flags = flags
        inner.apply = apply

        var processRequest = Confidence_Flags_Resolver_V1_ResolveProcessRequest()
        processRequest.resolve = .withoutMaterializations(inner)

        let reqPtr = try WasmMsg.transferRequest(processRequest, env: env)
        let result = try resolveFlagsFn.invoke([.i32(UInt32(bitPattern: reqPtr))])
        let respPtr = Int32(bitPattern: result[0].i32)
        let response = try WasmMsg.consumeResponse(respPtr, as: Confidence_Flags_Resolver_V1_ResolveProcessResponse.self, env: env)
        switch response.result {
        case .resolved(let resolved):
            return resolved.response
        case .suspended:
            throw LocalResolverError.unexpectedSuspend
        case nil:
            throw LocalResolverError.emptyResolveResult
        }
    }

    /// Logs an apply for previously-resolved flags. Best-effort; surfaces failures via
    /// the throw but apply-failures are non-fatal for the resolver.
    public func applyFlags(clientSecret: String, resolveToken: Data, flags: [Confidence_Flags_Resolver_V1_AppliedFlag]) throws {
        var request = Confidence_Flags_Resolver_V1_ApplyFlagsRequest()
        request.clientSecret = clientSecret
        request.resolveToken = resolveToken
        request.flags = flags

        let reqPtr = try WasmMsg.transferRequest(request, env: env)
        let result = try applyFlagsFn.invoke([.i32(UInt32(bitPattern: reqPtr))])
        let respPtr = Int32(bitPattern: result[0].i32)
        _ = try WasmMsg.consumeResponse(respPtr, as: Confidence_Wasm_Void.self, env: env)
    }
}

/// Errors raised by the local resolver.
public enum LocalResolverError: Swift.Error, CustomStringConvertible {
    case wasmResourceNotFound
    case missingExport(String)
    case unexpectedSuspend
    case emptyResolveResult

    public var description: String {
        switch self {
        case .wasmResourceNotFound: return "confidence_resolver.wasm not found in package resources"
        case .missingExport(let name): return "WASM module is missing required export: \(name)"
        case .unexpectedSuspend: return "WASM returned a Suspended result; materializations are not supported in unit-local mode"
        case .emptyResolveResult: return "WASM returned an empty ResolveProcessResponse"
        }
    }
}
