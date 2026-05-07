package com.spotify.confidence.android

import com.spotify.confidence.flags.resolver.v1.ResolveFlagsResponse
import com.spotify.confidence.flags.resolver.v1.ResolveWithStickyRequest
import com.spotify.confidence.flags.resolver.v1.ResolveWithStickyResponse
import java.util.concurrent.CompletableFuture
import java.util.concurrent.atomic.AtomicReference

/**
 * Swap-based resolver API that allows hot-swapping WASM resolver instances.
 *
 * This implementation:
 * - Creates new WASM instances with updated state
 * - Atomically swaps out old instances
 * - Handles retries for closed instances
 */
internal class SwapWasmResolverApi(
    private val flagLogger: WasmFlagLogger,
    initialState: ByteArray,
    accountId: String,
    private val materializationStore: MaterializationStore
) : ResolverApi {

    private val wasmResolverApiRef = AtomicReference<WasmResolver>()

    companion object {
        private const val MAX_CLOSED_RETRIES = 10
    }

    private fun createWasmResolver(): WasmResolver {
        return WasmResolver { logData -> flagLogger.write(logData) }
    }

    init {
        // Create initial instance
        val initialInstance = createWasmResolver()
        initialInstance.setResolverState(initialState, accountId)
        wasmResolverApiRef.set(initialInstance)
    }

    override fun init(state: ByteArray, accountId: String) {
        updateStateAndFlushLogs(state, accountId)
    }

    override fun isInitialized(): Boolean = true

    override fun updateStateAndFlushLogs(state: ByteArray, accountId: String) {
        // Create new instance with updated state
        val newInstance = createWasmResolver()
        newInstance.setResolverState(state, accountId)

        // Get current instance before switching
        val oldInstance = wasmResolverApiRef.getAndSet(newInstance)
        oldInstance?.close()
    }

    override fun close() {
        val currentInstance = wasmResolverApiRef.getAndSet(null)
        currentInstance?.close()
    }

    override fun resolveWithSticky(request: ResolveWithStickyRequest): CompletableFuture<ResolveFlagsResponse> {
        return resolveWithStickyInternal(request, 0)
    }

    private fun resolveWithStickyInternal(
        request: ResolveWithStickyRequest,
        closedRetries: Int
    ): CompletableFuture<ResolveFlagsResponse> {
        val instance = wasmResolverApiRef.get()
            ?: return CompletableFuture.failedFuture(RuntimeException("Resolver is closed"))

        val response = try {
            instance.resolveWithSticky(request)
        } catch (e: IsClosedException) {
            if (closedRetries >= MAX_CLOSED_RETRIES) {
                return CompletableFuture.failedFuture(
                    RuntimeException("Max retries exceeded for IsClosedException: $MAX_CLOSED_RETRIES", e)
                )
            }
            return resolveWithStickyInternal(request, closedRetries + 1)
        }

        return when (response.resolveResultCase) {
            ResolveWithStickyResponse.ResolveResultCase.SUCCESS -> {
                val success = response.success
                // For now, we skip materialization updates since the proto types aren't available
                // TODO: Add materialization support when proto types are generated
                CompletableFuture.completedFuture(success.response)
            }

            ResolveWithStickyResponse.ResolveResultCase.MISSING_MATERIALIZATIONS -> {
                // For now, return an error since we can't handle materializations
                // TODO: Add materialization support when proto types are generated
                android.util.Log.w("SwapWasmResolverApi", "Missing materializations - sticky assignments not supported yet")
                CompletableFuture.failedFuture(RuntimeException("Materialization support not implemented"))
            }

            ResolveWithStickyResponse.ResolveResultCase.RESOLVERESULT_NOT_SET ->
                CompletableFuture.failedFuture(RuntimeException("Invalid response: resolve result not set"))

            else ->
                CompletableFuture.failedFuture(RuntimeException("Unhandled response case: ${response.resolveResultCase}"))
        }
    }
}
