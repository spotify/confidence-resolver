import Foundation
import SwiftProtobuf
import WasmKit

/// Helpers that implement the `wasm-msg` protocol on the host side:
///
/// 1. `transfer(_:)` allocates guest memory, writes the serialized envelope, and returns
///    a guest-memory pointer (returned as an `Int32` for compatibility with WasmKit
///    `Value.i32`).
/// 2. After invoking a guest export, `consume(_:)` reads the size-prefixed envelope
///    written by the guest, calls `wasm_msg_free`, and returns the raw bytes.
///
/// On top of those primitives, `transferRequest(_:)` wraps an inner protobuf in the
/// `Confidence_Wasm_Request` envelope, and `consumeResponse(...)` unwraps the
/// `Confidence_Wasm_Response` envelope and returns the inner protobuf or throws on a
/// guest-reported error.
enum WasmMsg {
    /// Errors raised by the WASM message layer.
    enum Error: Swift.Error, CustomStringConvertible {
        case noMemoryExport
        case allocReturnedNull
        case nullResponsePointer
        case memoryReadOutOfBounds(offset: UInt32, count: Int, memorySize: Int)
        case guestError(String)

        var description: String {
            switch self {
            case .noMemoryExport: return "WASM module does not export a memory named 'memory'"
            case .allocReturnedNull: return "wasm_msg_alloc returned a null pointer"
            case .nullResponsePointer: return "WASM export returned a null response pointer"
            case let .memoryReadOutOfBounds(offset, count, memorySize):
                return "Memory read out of bounds: offset=\(offset) count=\(count) memorySize=\(memorySize)"
            case let .guestError(message): return "WASM guest error: \(message)"
            }
        }
    }

    /// Wraps the given inner protobuf in a `Confidence_Wasm_Request` envelope, allocates
    /// guest memory for the serialized bytes, copies them in, and returns the guest pointer.
    static func transferRequest<M: SwiftProtobuf.Message>(_ message: M, env: WasmEnv) throws -> Int32 {
        let innerBytes = try message.serializedData()
        var envelope = Confidence_Wasm_Request()
        envelope.data = innerBytes
        let envelopeBytes = try envelope.serializedData()
        return try transfer(bytes: envelopeBytes, env: env)
    }

    /// Reads the `Confidence_Wasm_Response` envelope at `responsePtr`, frees the guest
    /// memory, and either returns the parsed inner protobuf or throws the guest-reported
    /// error string.
    static func consumeResponse<M: SwiftProtobuf.Message>(_ responsePtr: Int32, as: M.Type, env: WasmEnv) throws -> M {
        let bytes = try consume(ptr: responsePtr, env: env)
        let response = try Confidence_Wasm_Response(serializedBytes: bytes)
        switch response.result {
        case .data(let inner): return try M(serializedBytes: inner)
        case .error(let message): throw Error.guestError(message)
        case nil: throw Error.guestError("response envelope had no result")
        }
    }

    /// Allocates `bytes.count` bytes of guest memory via `wasm_msg_alloc`, writes the
    /// bytes, and returns the guest pointer.
    static func transfer(bytes: Data, env: WasmEnv) throws -> Int32 {
        guard let alloc = env.alloc, let memory = env.memory else {
            throw Error.noMemoryExport
        }
        let result = try alloc.invoke([.i32(UInt32(bytes.count))])
        let ptr = result[0].i32
        guard ptr != 0 else { throw Error.allocReturnedNull }
        try writeMemory(memory: memory, offset: ptr, bytes: bytes)
        return Int32(bitPattern: ptr)
    }

    /// Reads the size-prefixed envelope at `ptr`, calls `wasm_msg_free`, and returns
    /// the data bytes (excluding the 4-byte length prefix the guest wrote at `ptr - 4`).
    static func consume(ptr: Int32, env: WasmEnv) throws -> Data {
        guard ptr != 0 else { throw Error.nullResponsePointer }
        guard let memory = env.memory, let free = env.free else {
            throw Error.noMemoryExport
        }
        let unsignedPtr = UInt32(bitPattern: ptr)
        let totalSize = try readUInt32(memory: memory, offset: unsignedPtr &- 4)
        let dataLength = Int(totalSize) - MemoryLayout<UInt32>.size
        let bytes = try readMemory(memory: memory, offset: unsignedPtr, count: dataLength)
        _ = try free.invoke([.i32(unsignedPtr)])
        return bytes
    }

    /// Wraps an inner protobuf result coming from the host (we, as the host, are
    /// responding to a guest-initiated import) in a `Confidence_Wasm_Response` envelope,
    /// allocates guest memory, and returns the pointer. Used by host-import closures.
    static func transferResponseSuccess<M: SwiftProtobuf.Message>(_ inner: M, env: WasmEnv) throws -> Int32 {
        let innerBytes = try inner.serializedData()
        var envelope = Confidence_Wasm_Response()
        envelope.result = .data(innerBytes)
        let envelopeBytes = try envelope.serializedData()
        return try transfer(bytes: envelopeBytes, env: env)
    }

    static func transferResponseError(_ message: String, env: WasmEnv) throws -> Int32 {
        var envelope = Confidence_Wasm_Response()
        envelope.result = .error(message)
        let envelopeBytes = try envelope.serializedData()
        return try transfer(bytes: envelopeBytes, env: env)
    }

    /// Reads the `Confidence_Wasm_Request` envelope at `ptr` (sent to us by the guest),
    /// frees the guest memory, and returns the inner protobuf parsed as `M`.
    static func consumeRequest<M: SwiftProtobuf.Message>(_ ptr: Int32, as: M.Type, env: WasmEnv) throws -> M {
        let bytes = try consume(ptr: ptr, env: env)
        let request = try Confidence_Wasm_Request(serializedBytes: bytes)
        return try M(serializedBytes: request.data)
    }

    private static func writeMemory(memory: Memory, offset: UInt32, bytes: Data) throws {
        memory.withUnsafeMutableBufferPointer(offset: UInt(offset), count: bytes.count) { buffer in
            _ = bytes.copyBytes(to: buffer)
        }
    }

    private static func readMemory(memory: Memory, offset: UInt32, count: Int) throws -> Data {
        memory.withUnsafeMutableBufferPointer(offset: UInt(offset), count: count) { buffer in
            Data(bytes: buffer.baseAddress!, count: count)
        }
    }

    private static func readUInt32(memory: Memory, offset: UInt32) throws -> UInt32 {
        memory.withUnsafeMutableBufferPointer(offset: UInt(offset), count: 4) { buffer in
            buffer.load(as: UInt32.self).littleEndian
        }
    }
}
