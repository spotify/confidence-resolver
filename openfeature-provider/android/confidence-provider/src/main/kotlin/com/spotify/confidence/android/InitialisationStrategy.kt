package com.spotify.confidence.android

/**
 * Strategy for provider initialization.
 *
 * Determines how the provider behaves during initialization:
 * - [FetchAndActivate]: Fetches fresh state from CDN and waits for completion before becoming ready.
 *   Use this when you want guaranteed fresh flags at startup.
 * - [ActivateAndFetchAsync]: Immediately activates with cached state and fetches updates in background.
 *   Use this for faster startup when stale values are acceptable.
 */
sealed interface InitialisationStrategy {
    /**
     * Fetch fresh state from CDN and activate it before becoming ready.
     * Provides guaranteed fresh flags but may increase startup latency.
     */
    object FetchAndActivate : InitialisationStrategy

    /**
     * Immediately activate cached state and fetch updates asynchronously.
     * Faster startup but flags may be stale until fetch completes.
     */
    object ActivateAndFetchAsync : InitialisationStrategy
}
