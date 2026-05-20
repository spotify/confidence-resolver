// NOTE(unit-local experiment): An OpenFeature FeatureProvider implementation
// that resolves flags via the embedded WASM resolver and fetches per-unit
// slices from a local slice server. Mirrors the role of the Java
// `OpenFeatureLocalResolveProvider`, but local-only (no remote fallback,
// no materialization store, no flag-log pubsub) — this is a POC.

import Combine
import ConfidenceLocalResolver
import Foundation
import OpenFeature
import SwiftProtobuf

public final class ConfidenceLocalProvider: FeatureProvider {
    public let metadata: ProviderMetadata = ConfidenceLocalProviderMetadata()
    public var hooks: [any Hook] = []

    private let clientSecret: String
    private let serverURL: URL
    private let sliceClient: SliceClient
    private let resolver: LocalResolver
    private let eventHandler = EventHandler()

    private let stateLock = NSLock()
    private var bootstrapped = false
    private var accountId: String = ""
    private var stateFileHash: String = ""
    private var randomizationUnitFields: [String] = []
    private var lastUnit: String?
    private var resolvedFlags: [String: Confidence_Flags_Resolver_V1_ResolvedFlag] = [:]
    private var resolveToken: Data = Data()

    public init(clientSecret: String, serverURL: URL = URL(string: "http://127.0.0.1:8787")!) throws {
        self.clientSecret = clientSecret
        self.serverURL = serverURL
        self.sliceClient = SliceClient(baseURL: serverURL)
        self.resolver = try LocalResolver()
    }

    public func observe() -> AnyPublisher<ProviderEvent?, Never> {
        eventHandler.observe()
    }

    public func initialize(initialContext: EvaluationContext?) async throws {
        let config = try await sliceClient.accountConfig()
        storeBootstrap(config: config)

        if let context = initialContext {
            try await applyContext(context)
        }
        eventHandler.send(.ready())
    }

    private func storeBootstrap(config: SliceClient.AccountConfig) {
        stateLock.lock()
        defer { stateLock.unlock() }
        accountId = config.accountId
        stateFileHash = config.stateFileHash
        randomizationUnitFields = config.randomizationUnitFields
        bootstrapped = true
    }

    public func onContextSet(oldContext: EvaluationContext?, newContext: EvaluationContext) async throws {
        try await applyContext(newContext)
    }

    private func applyContext(_ context: EvaluationContext) async throws {
        guard readBootstrapped() else { throw OpenFeatureError.providerNotReadyError }

        let unit = try extractUnit(from: context)
        let currentUnit = readLastUnit()
        let (accountIdCopy, stateFileHashCopy) = readAccountAndHash()

        if currentUnit != unit {
            let slice = try await sliceClient.fetchSlice(stateFileHash: stateFileHashCopy, unit: unit)
            try resolver.setResolverState(slice.bytes, accountId: accountIdCopy)
            storeLastUnit(unit)
        }

        let pbContext = OpenFeatureConverters.toProtoStruct(context)
        let response = try resolver.resolveFlags(
            clientSecret: clientSecret,
            evaluationContext: pbContext,
            apply: false
        )

        let resolved = Dictionary(uniqueKeysWithValues: response.resolvedFlags.map { (stripFlagsPrefix($0.flag), $0) })
        let token = response.resolveToken
        storeResolved(flags: resolved, token: token)

        // Fire-and-forget apply for the matched flags (open question #6 follow-up).
        let appliedFlags: [Confidence_Flags_Resolver_V1_AppliedFlag] = response.resolvedFlags
            .filter { $0.reason == .match }
            .map { rf in
                var af = Confidence_Flags_Resolver_V1_AppliedFlag()
                af.flag = rf.flag
                af.applyTime = .init(date: Date())
                return af
            }
        if !appliedFlags.isEmpty && !token.isEmpty {
            Task.detached { [sliceClient, clientSecret, token, appliedFlags] in
                do {
                    try await sliceClient.applyFlags(
                        clientSecret: clientSecret,
                        resolveToken: token,
                        flags: appliedFlags
                    )
                } catch {
                    // Best-effort; align with the SDK's existing "apply is fire-and-forget" semantics.
                }
            }
        }
    }

    private func extractUnit(from context: EvaluationContext) throws -> String {
        let fields = readRandomizationUnitFields()
        let map = context.asMap()
        for field in fields {
            if let v = map[field], let s = v.asString(), !s.isEmpty {
                return s
            }
        }
        // Fall back to the OpenFeature targetingKey if the slice server's
        // randomization fields aren't populated directly.
        let targetingKey = context.getTargetingKey()
        if !targetingKey.isEmpty {
            return targetingKey
        }
        throw OpenFeatureError.generalError(
            message: "Evaluation context must contain a non-empty value for one of: \(fields), or a targetingKey"
        )
    }

    // MARK: - Lock-protected accessors (sync helpers so the async paths above
    // don't hold a non-async-safe NSLock across an `await`).

    private func readBootstrapped() -> Bool {
        stateLock.lock(); defer { stateLock.unlock() }
        return bootstrapped
    }

    private func readLastUnit() -> String? {
        stateLock.lock(); defer { stateLock.unlock() }
        return lastUnit
    }

    private func readAccountAndHash() -> (String, String) {
        stateLock.lock(); defer { stateLock.unlock() }
        return (accountId, stateFileHash)
    }

    private func readRandomizationUnitFields() -> [String] {
        stateLock.lock(); defer { stateLock.unlock() }
        return randomizationUnitFields
    }

    private func storeLastUnit(_ unit: String) {
        stateLock.lock(); defer { stateLock.unlock() }
        lastUnit = unit
    }

    private func storeResolved(flags: [String: Confidence_Flags_Resolver_V1_ResolvedFlag], token: Data) {
        stateLock.lock(); defer { stateLock.unlock() }
        resolvedFlags = flags
        resolveToken = token
    }

    // MARK: - Evaluation

    public func getBooleanEvaluation(key: String, defaultValue: Bool, context: EvaluationContext?) throws
        -> ProviderEvaluation<Bool>
    {
        try evaluate(key: key, defaultValue: defaultValue, extractor: { $0.asBoolean() })
    }

    public func getStringEvaluation(key: String, defaultValue: String, context: EvaluationContext?) throws
        -> ProviderEvaluation<String>
    {
        try evaluate(key: key, defaultValue: defaultValue, extractor: { $0.asString() })
    }

    public func getIntegerEvaluation(key: String, defaultValue: Int64, context: EvaluationContext?) throws
        -> ProviderEvaluation<Int64>
    {
        try evaluate(key: key, defaultValue: defaultValue, extractor: { $0.asInteger() })
    }

    public func getDoubleEvaluation(key: String, defaultValue: Double, context: EvaluationContext?) throws
        -> ProviderEvaluation<Double>
    {
        try evaluate(key: key, defaultValue: defaultValue, extractor: { $0.asDouble() })
    }

    public func getObjectEvaluation(key: String, defaultValue: Value, context: EvaluationContext?) throws
        -> ProviderEvaluation<Value>
    {
        try evaluate(key: key, defaultValue: defaultValue, extractor: { $0 as Value? })
    }

    private func evaluate<T>(
        key: String,
        defaultValue: T,
        extractor: (Value) -> T?
    ) throws -> ProviderEvaluation<T> {
        let (flagName, path) = parseKey(key)
        stateLock.lock()
        let resolved = resolvedFlags[flagName]
        stateLock.unlock()

        guard let resolved else {
            throw OpenFeatureError.flagNotFoundError(key: key)
        }

        let reason = OpenFeatureConverters.reason(resolved.reason)
        switch resolved.reason {
        case .targetingKeyError:
            return ProviderEvaluation(
                value: defaultValue,
                variant: nil,
                reason: reason,
                errorCode: .targetingKeyMissing,
                errorMessage: "Invalid targeting key"
            )
        case .error, .unspecified:
            return ProviderEvaluation(
                value: defaultValue,
                variant: nil,
                reason: reason,
                errorCode: .general,
                errorMessage: "Resolve error"
            )
        case .flagArchived, .noSegmentMatch, .noTreatmentMatch:
            return ProviderEvaluation(
                value: defaultValue,
                variant: resolved.variant.isEmpty ? nil : resolved.variant,
                reason: reason
            )
        case .match:
            break  // proceed below
        default:
            return ProviderEvaluation(value: defaultValue, variant: nil, reason: reason)
        }

        let asValue = OpenFeatureConverters.toValue(.with { $0.structValue = resolved.value })
        let drilled: Value
        do {
            drilled = try drill(value: asValue, path: path)
        } catch {
            throw OpenFeatureError.parseError(message: "Unable to resolve path '\(path.joined(separator: "."))' on flag '\(flagName)': \(error)")
        }

        guard let typed = extractor(drilled) else {
            throw OpenFeatureError.typeMismatchError
        }

        return ProviderEvaluation(
            value: typed,
            variant: resolved.variant.isEmpty ? nil : resolved.variant,
            reason: reason
        )
    }

    private func drill(value: Value, path: [String]) throws -> Value {
        var current = value
        for step in path {
            guard case .structure(let map) = current, let next = map[step] else {
                throw OpenFeatureError.parseError(message: "key '\(step)' not found")
            }
            current = next
        }
        return current
    }

    private func parseKey(_ key: String) -> (String, [String]) {
        let parts = key.split(separator: ".", omittingEmptySubsequences: true).map(String.init)
        guard let first = parts.first else { return (key, []) }
        return (first, Array(parts.dropFirst()))
    }
}

private func stripFlagsPrefix(_ raw: String) -> String {
    raw.hasPrefix("flags/") ? String(raw.dropFirst("flags/".count)) : raw
}

private struct ConfidenceLocalProviderMetadata: ProviderMetadata {
    let name: String? = "ConfidenceLocalProvider"
}

// MARK: - OpenFeature <-> proto / SDK value converters

enum OpenFeatureConverters {
    static func toProtoStruct(_ ctx: EvaluationContext) -> Google_Protobuf_Struct {
        var s = Google_Protobuf_Struct()
        s.fields = ctx.asMap().mapValues { toProtoValue($0) }
        // OpenFeature exposes the targeting key as a separate accessor; bring
        // it into the proto context so downstream selectors can see it.
        let targetingKey = ctx.getTargetingKey()
        if !targetingKey.isEmpty, s.fields["targeting_key"] == nil {
            s.fields["targeting_key"] = .with { $0.stringValue = targetingKey }
        }
        return s
    }

    static func toProtoValue(_ value: Value) -> Google_Protobuf_Value {
        switch value {
        case .boolean(let b):
            return .with { $0.boolValue = b }
        case .string(let s):
            return .with { $0.stringValue = s }
        case .integer(let i):
            return .with { $0.numberValue = Double(i) }
        case .double(let d):
            return .with { $0.numberValue = d }
        case .date(let date):
            let f = ISO8601DateFormatter()
            f.timeZone = TimeZone(identifier: "UTC")
            return .with { $0.stringValue = f.string(from: date) }
        case .list(let arr):
            let elements = arr.map { toProtoValue($0) }
            return .with { $0.listValue = Google_Protobuf_ListValue.with { $0.values = elements } }
        case .structure(let map):
            var inner = Google_Protobuf_Struct()
            inner.fields = map.mapValues { toProtoValue($0) }
            return .with { $0.structValue = inner }
        case .null:
            return .with { $0.nullValue = .nullValue }
        }
    }

    static func toValue(_ pb: Google_Protobuf_Value) -> Value {
        switch pb.kind {
        case .boolValue(let b): return .boolean(b)
        case .stringValue(let s): return .string(s)
        case .numberValue(let n):
            if n.truncatingRemainder(dividingBy: 1) == 0, n >= Double(Int64.min), n <= Double(Int64.max) {
                return .integer(Int64(n))
            }
            return .double(n)
        case .nullValue: return .null
        case .listValue(let list): return .list(list.values.map(toValue))
        case .structValue(let s): return .structure(s.fields.mapValues(toValue))
        case nil: return .null
        }
    }

    static func reason(_ pb: Confidence_Flags_Resolver_V1_ResolveReason) -> String {
        switch pb {
        case .unspecified: return "UNSPECIFIED"
        case .match: return "MATCH"
        case .noSegmentMatch: return "NO_SEGMENT_MATCH"
        case .noTreatmentMatch: return "NO_TREATMENT_MATCH"
        case .flagArchived: return "FLAG_ARCHIVED"
        case .targetingKeyError: return "TARGETING_KEY_ERROR"
        case .error: return "ERROR"
        default: return "UNKNOWN"
        }
    }
}
