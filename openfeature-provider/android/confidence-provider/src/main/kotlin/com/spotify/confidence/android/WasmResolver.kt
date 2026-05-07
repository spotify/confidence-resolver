package com.spotify.confidence.android

import com.dylibso.chicory.runtime.ExportFunction
import com.dylibso.chicory.runtime.ImportFunction
import com.dylibso.chicory.runtime.ImportValues
import com.dylibso.chicory.runtime.Instance
import com.dylibso.chicory.runtime.Memory
import com.dylibso.chicory.wasm.types.ValType
import com.spotify.confidence.wasm.ConfidenceResolver
import com.google.protobuf.ByteString
import com.google.protobuf.InvalidProtocolBufferException
import com.google.protobuf.MessageLite
import com.google.protobuf.Timestamp
import com.spotify.confidence.flags.resolver.v1.ResolveWithStickyRequest
import com.spotify.confidence.flags.resolver.v1.ResolveWithStickyResponse
import rust_guest.LogMessage
import rust_guest.Request
import rust_guest.Response
import rust_guest.SetResolverStateRequest
import rust_guest.Void as WasmVoid
import java.time.Instant
import java.util.concurrent.locks.ReentrantReadWriteLock

/**
 * WASM resolver that interfaces with the Confidence resolver WASM binary using Chicory runtime.
 *
 * This class handles all WASM memory management and provides methods for:
 * - Setting resolver state
 * - Resolving flags with sticky assignments
 * - Flushing logs
 */
internal class WasmResolver(
    private val onFlushLogs: (ByteArray) -> Unit
) {
    private val hostParamTypes = listOf(ValType.I32)
    private val hostReturnTypes = listOf(ValType.I32)
    private val instance: Instance
    private val wasmLock = ReentrantReadWriteLock()

    @Volatile
    private var isConsumed = false

    // WASM memory interop functions
    private val wasmMsgAlloc: ExportFunction
    private val wasmMsgFree: ExportFunction

    // WASM API functions
    private val wasmMsgGuestSetResolverState: ExportFunction
    private val wasmMsgFlushLogs: ExportFunction
    private val wasmMsgGuestResolveWithSticky: ExportFunction

    init {
        // Use pre-compiled WASM module for better performance on Android
        // The module is compiled at build time using Chicory's build-time compiler
        val module = ConfidenceResolver.load()
        instance = Instance.builder(module)
            .withImportValues(
                ImportValues.builder()
                    .addFunction(createImportFunction("current_time", WasmVoid::parseFrom, ::currentTime))
                    .addFunction(createImportFunction("log_message", LogMessage::parseFrom, ::log))
                    .addFunction(
                        ImportFunction(
                            "wasm_msg",
                            "wasm_msg_current_thread_id",
                            emptyList<ValType>(),
                            listOf(ValType.I32)
                        ) { _, _ -> longArrayOf(0) }
                    )
                    .build()
            )
            // Use pre-compiled machine factory for fast execution
            .withMachineFactory(ConfidenceResolver::create)
            .build()

        wasmMsgAlloc = instance.export("wasm_msg_alloc")
        wasmMsgFree = instance.export("wasm_msg_free")
        wasmMsgGuestSetResolverState = instance.export("wasm_msg_guest_set_resolver_state")
        wasmMsgFlushLogs = instance.export("wasm_msg_guest_flush_logs")
        wasmMsgGuestResolveWithSticky = instance.export("wasm_msg_guest_resolve_with_sticky")
    }

    private fun log(message: LogMessage): MessageLite {
        android.util.Log.d("WasmResolver", message.message)
        return WasmVoid.getDefaultInstance()
    }

    private fun currentTime(@Suppress("UNUSED_PARAMETER") unused: WasmVoid): Timestamp {
        return Timestamp.newBuilder()
            .setSeconds(Instant.now().epochSecond)
            .build()
    }

    /**
     * Sets the resolver state from CDN.
     */
    fun setResolverState(state: ByteArray, accountId: String) {
        val resolverStateRequest = SetResolverStateRequest.newBuilder()
            .setState(ByteString.copyFrom(state))
            .setAccountId(accountId)
            .build()

        val request = Request.newBuilder()
            .setData(ByteString.copyFrom(resolverStateRequest.toByteArray()))
            .build()
            .toByteArray()

        val addr = transfer(request)
        val respPtr = wasmMsgGuestSetResolverState.apply(addr.toLong())[0].toInt()
        consumeResponse(respPtr) { WasmVoid.parseFrom(it) }
    }

    /**
     * Resolves flags with sticky assignment support.
     */
    @Throws(IsClosedException::class)
    fun resolveWithSticky(request: ResolveWithStickyRequest): ResolveWithStickyResponse {
        if (!wasmLock.writeLock().tryLock() || isConsumed) {
            throw IsClosedException()
        }
        try {
            val reqPtr = transferRequest(request)
            val respPtr = wasmMsgGuestResolveWithSticky.apply(reqPtr.toLong())[0].toInt()
            return consumeResponse(respPtr) { ResolveWithStickyResponse.parseFrom(it) }
        } finally {
            wasmLock.writeLock().unlock()
        }
    }

    /**
     * Flushes pending logs and closes the resolver.
     */
    fun close() {
        wasmLock.readLock().lock()
        try {
            val voidRequest = WasmVoid.getDefaultInstance()
            val reqPtr = transferRequest(voidRequest)
            val respPtr = wasmMsgFlushLogs.apply(reqPtr.toLong())[0].toInt()
            val responseBytes = consumeResponseBytes(respPtr)
            onFlushLogs(responseBytes)
            isConsumed = true
        } finally {
            wasmLock.readLock().unlock()
        }
    }

    private fun <T : MessageLite> consumeResponse(addr: Int, codec: (ByteArray) -> T): T {
        try {
            val response = Response.parseFrom(consume(addr))
            if (response.hasError()) {
                throw RuntimeException(response.error)
            }
            return codec(response.data.toByteArray())
        } catch (e: InvalidProtocolBufferException) {
            throw RuntimeException(e)
        }
    }

    private fun consumeResponseBytes(addr: Int): ByteArray {
        try {
            val response = Response.parseFrom(consume(addr))
            if (response.hasError()) {
                throw RuntimeException(response.error)
            }
            return response.data.toByteArray()
        } catch (e: InvalidProtocolBufferException) {
            throw RuntimeException(e)
        }
    }

    private fun transferRequest(message: MessageLite): Int {
        val request = Request.newBuilder()
            .setData(ByteString.copyFrom(message.toByteArray()))
            .build()
            .toByteArray()
        return transfer(request)
    }

    private fun transferResponseSuccess(response: MessageLite): Int {
        val wrapperBytes = Response.newBuilder()
            .setData(ByteString.copyFrom(response.toByteArray()))
            .build()
            .toByteArray()
        return transfer(wrapperBytes)
    }

    private fun transferResponseError(error: String): Int {
        val wrapperBytes = Response.newBuilder()
            .setError(error)
            .build()
            .toByteArray()
        return transfer(wrapperBytes)
    }

    private fun consume(addr: Int): ByteArray {
        val mem: Memory = instance.memory()
        val len = (mem.readU32(addr - 4) - 4L).toInt()
        val data = mem.readBytes(addr, len)
        wasmMsgFree.apply(addr.toLong())
        return data
    }

    private fun transfer(data: ByteArray): Int {
        val mem: Memory = instance.memory()
        val addr = wasmMsgAlloc.apply(data.size.toLong())[0].toInt()
        mem.write(addr, data)
        return addr
    }

    private fun <T : MessageLite> consumeRequest(addr: Int, codec: (ByteArray) -> T): T {
        try {
            val request = Request.parseFrom(consume(addr))
            return codec(request.data.toByteArray())
        } catch (e: InvalidProtocolBufferException) {
            throw RuntimeException(e)
        }
    }

    private fun <T : MessageLite> createImportFunction(
        name: String,
        reqCodec: (ByteArray) -> T,
        impl: (T) -> MessageLite
    ): ImportFunction {
        return ImportFunction(
            "wasm_msg",
            "wasm_msg_host_$name",
            hostParamTypes,
            hostReturnTypes
        ) { _, args ->
            try {
                val message = consumeRequest(args[0].toInt(), reqCodec)
                val response = impl(message)
                longArrayOf(transferResponseSuccess(response).toLong())
            } catch (e: Exception) {
                longArrayOf(transferResponseError(e.message ?: "Unknown error").toLong())
            }
        }
    }
}

/**
 * Exception thrown when the resolver is closed and cannot process more requests.
 */
class IsClosedException : Exception("Resolver is closed")
