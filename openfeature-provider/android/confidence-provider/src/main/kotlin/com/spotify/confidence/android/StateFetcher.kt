package com.spotify.confidence.android

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import okhttp3.Request
import rust_guest.SetResolverStateRequest
import java.nio.charset.StandardCharsets
import java.security.MessageDigest
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/**
 * Fetches and caches resolver state from the Confidence CDN.
 *
 * This class handles:
 * - Fetching state from the CDN using SHA256(clientSecret) as the path
 * - ETag-based conditional GETs to minimize bandwidth
 * - Parsing SetResolverStateRequest protobuf from the response
 */
internal class StateFetcher(
    private val clientSecret: String,
    private val httpClient: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(30, TimeUnit.SECONDS)
        .readTimeout(30, TimeUnit.SECONDS)
        .build()
) {
    private val etagHolder = AtomicReference<String?>(null)
    private val rawResolverStateHolder = AtomicReference<ByteArray>(ByteArray(0))
    private var accountId: String = ""

    companion object {
        private const val CDN_BASE_URL = "https://confidence-resolver-state-cdn.spotifycdn.com/"
    }

    /**
     * Returns the current cached resolver state.
     */
    fun provide(): ByteArray = rawResolverStateHolder.get()

    /**
     * Returns the current account ID.
     */
    fun accountId(): String = accountId

    /**
     * Reloads the resolver state from the CDN.
     * Uses ETag for conditional GET to avoid re-downloading unchanged state.
     */
    suspend fun reload() {
        withContext(Dispatchers.IO) {
            try {
                fetchAndUpdateStateIfChanged()
            } catch (e: Exception) {
                android.util.Log.w("StateFetcher", "Failed to reload, ignoring reload", e)
            }
        }
    }

    private fun fetchAndUpdateStateIfChanged() {
        val cdnUrl = CDN_BASE_URL + sha256Hex(clientSecret)

        val requestBuilder = Request.Builder().url(cdnUrl)
        etagHolder.get()?.let { previousEtag ->
            requestBuilder.header("If-None-Match", previousEtag)
        }

        val response = httpClient.newCall(requestBuilder.build()).execute()
        response.use { resp ->
            if (resp.code == 304) {
                // Not modified
                return
            }

            if (!resp.isSuccessful) {
                throw RuntimeException("Failed to fetch state: HTTP ${resp.code}")
            }

            val etag = resp.header("ETag")
            val bytes = resp.body?.bytes() ?: throw RuntimeException("Empty response body")

            // Parse SetResolverStateRequest from CDN response
            val stateRequest = SetResolverStateRequest.parseFrom(bytes)
            this.accountId = stateRequest.accountId

            // Store the state bytes
            rawResolverStateHolder.set(stateRequest.state.toByteArray())
            etagHolder.set(etag)

            android.util.Log.i("StateFetcher", "Loaded resolver state for account=$accountId, etag=$etag")
        }
    }

    private fun sha256Hex(input: String): String {
        val digest = MessageDigest.getInstance("SHA-256")
        val hash = digest.digest(input.toByteArray(StandardCharsets.UTF_8))
        return hash.joinToString("") { byte ->
            String.format("%02x", byte)
        }
    }
}
