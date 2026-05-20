import Foundation
import SwiftProtobuf

/// Fetches per-unit slices from the unit-local slice server.
public struct SliceClient {
    public struct AccountConfig: Decodable, Sendable {
        public let accountId: String
        public let stateFileHash: String
        public let randomizationUnitFields: [String]
        public let fullStateSizeBytes: Int

        private enum CodingKeys: String, CodingKey {
            case accountId = "account_id"
            case stateFileHash = "state_file_hash"
            case randomizationUnitFields = "randomization_unit_fields"
            case fullStateSizeBytes = "full_state_size_bytes"
        }
    }

    public struct Slice: Sendable {
        public let bytes: Data
        public let stateFileHash: String
        public let randomizationUnitFields: [String]
    }

    public enum Error: Swift.Error, CustomStringConvertible {
        case http(status: Int, body: String)
        case missingHeader(String)

        public var description: String {
            switch self {
            case let .http(status, body): return "HTTP \(status): \(body)"
            case let .missingHeader(name): return "Slice response missing header: \(name)"
            }
        }
    }

    public let baseURL: URL
    public let session: URLSession

    public init(baseURL: URL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    public func accountConfig() async throws -> AccountConfig {
        let url = baseURL.appendingPathComponent("/v1/account-config")
        let (data, response) = try await session.data(from: url)
        try Self.requireSuccess(response: response, body: data)
        let decoder = JSONDecoder()
        return try decoder.decode(AccountConfig.self, from: data)
    }

    public func fetchSlice(stateFileHash: String, unit: String) async throws -> Slice {
        let path = "/v1/resolver-state/\(stateFileHash)/\(unit)"
        let url = baseURL.appendingPathComponent(path)
        let (data, response) = try await session.data(from: url)
        try Self.requireSuccess(response: response, body: data)
        guard let httpResponse = response as? HTTPURLResponse else {
            throw Error.http(status: 0, body: "non-HTTP response")
        }
        let fieldsHeader = (httpResponse.value(forHTTPHeaderField: "X-Randomization-Unit-Fields")
                            ?? httpResponse.value(forHTTPHeaderField: "x-randomization-unit-fields") ?? "")
        let hash = (httpResponse.value(forHTTPHeaderField: "X-State-File-Hash")
                    ?? httpResponse.value(forHTTPHeaderField: "x-state-file-hash") ?? stateFileHash)
        let fields = fieldsHeader
            .split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        return Slice(bytes: data, stateFileHash: hash, randomizationUnitFields: fields)
    }

    /// Fire-and-forget POST of an `ApplyFlagsRequest` to the slice server. Production
    /// would forward this onto the real apply backend; the prototype's server
    /// just ack-logs it.
    public func applyFlags(
        clientSecret: String,
        resolveToken: Data,
        flags: [Confidence_Flags_Resolver_V1_AppliedFlag]
    ) async throws {
        var req = Confidence_Flags_Resolver_V1_ApplyFlagsRequest()
        req.clientSecret = clientSecret
        req.resolveToken = resolveToken
        req.flags = flags
        req.sendTime = .init(date: Date())

        let body = try req.serializedData()
        let url = baseURL.appendingPathComponent("/v1/apply")
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/protobuf", forHTTPHeaderField: "Content-Type")
        request.httpBody = body

        let (data, response) = try await session.data(for: request)
        try Self.requireSuccess(response: response, body: data)
    }

    private static func requireSuccess(response: URLResponse, body: Data) throws {
        guard let httpResponse = response as? HTTPURLResponse else {
            throw Error.http(status: 0, body: "non-HTTP response")
        }
        guard (200..<300).contains(httpResponse.statusCode) else {
            let bodyText = String(data: body, encoding: .utf8) ?? "<binary>"
            throw Error.http(status: httpResponse.statusCode, body: bodyText)
        }
    }
}
