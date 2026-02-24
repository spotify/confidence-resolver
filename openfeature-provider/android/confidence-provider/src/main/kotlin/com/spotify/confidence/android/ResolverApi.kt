package com.spotify.confidence.android

import com.spotify.confidence.flags.resolver.v1.ResolveFlagsResponse
import com.spotify.confidence.flags.resolver.v1.ResolveWithStickyRequest
import java.util.concurrent.CompletableFuture

/**
 * Interface for the resolver API that handles flag resolution.
 */
internal interface ResolverApi {
    /**
     * Initializes the resolver with the given state and account ID.
     */
    fun init(state: ByteArray, accountId: String)

    /**
     * Returns whether the resolver has been initialized.
     */
    fun isInitialized(): Boolean

    /**
     * Updates the resolver state and flushes any pending logs.
     */
    fun updateStateAndFlushLogs(state: ByteArray, accountId: String)

    /**
     * Resolves flags with sticky assignment support.
     */
    fun resolveWithSticky(request: ResolveWithStickyRequest): CompletableFuture<ResolveFlagsResponse>

    /**
     * Closes the resolver, flushing any pending logs.
     */
    fun close()
}
