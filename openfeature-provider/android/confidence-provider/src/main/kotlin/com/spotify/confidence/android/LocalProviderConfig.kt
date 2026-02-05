package com.spotify.confidence.android

import okhttp3.OkHttpClient
import java.time.Duration

/**
 * Configuration for the Confidence local OpenFeature provider.
 *
 * @property channelFactory Factory for creating gRPC channels (useful for testing)
 * @property httpClient Custom OkHttpClient for CDN requests
 * @property useRemoteMaterializationStore Whether to use remote gRPC for sticky assignments
 * @property statePollInterval How often to poll for state updates
 * @property logFlushInterval How often to flush flag logs
 * @property initialisationStrategy Strategy for provider initialization
 */
data class LocalProviderConfig(
    val channelFactory: ChannelFactory = DefaultChannelFactory(),
    val httpClient: OkHttpClient? = null,
    val useRemoteMaterializationStore: Boolean = false,
    val statePollInterval: Duration = DEFAULT_POLL_INTERVAL,
    val logFlushInterval: Duration = DEFAULT_LOG_FLUSH_INTERVAL,
    val initialisationStrategy: InitialisationStrategy = InitialisationStrategy.FetchAndActivate
) {
    companion object {
        val DEFAULT_POLL_INTERVAL: Duration = Duration.ofSeconds(30)
        val DEFAULT_LOG_FLUSH_INTERVAL: Duration = Duration.ofSeconds(10)
    }

    /**
     * Builder for creating [LocalProviderConfig] instances.
     */
    class Builder {
        private var channelFactory: ChannelFactory = DefaultChannelFactory()
        private var httpClient: OkHttpClient? = null
        private var useRemoteMaterializationStore: Boolean = false
        private var statePollInterval: Duration = DEFAULT_POLL_INTERVAL
        private var logFlushInterval: Duration = DEFAULT_LOG_FLUSH_INTERVAL
        private var initialisationStrategy: InitialisationStrategy = InitialisationStrategy.FetchAndActivate

        fun channelFactory(factory: ChannelFactory) = apply { channelFactory = factory }
        fun httpClient(client: OkHttpClient) = apply { httpClient = client }
        fun useRemoteMaterializationStore(use: Boolean) = apply { useRemoteMaterializationStore = use }
        fun statePollInterval(interval: Duration) = apply { statePollInterval = interval }
        fun logFlushInterval(interval: Duration) = apply { logFlushInterval = interval }
        fun initialisationStrategy(strategy: InitialisationStrategy) = apply { initialisationStrategy = strategy }

        fun build(): LocalProviderConfig = LocalProviderConfig(
            channelFactory = channelFactory,
            httpClient = httpClient,
            useRemoteMaterializationStore = useRemoteMaterializationStore,
            statePollInterval = statePollInterval,
            logFlushInterval = logFlushInterval,
            initialisationStrategy = initialisationStrategy
        )
    }
}
