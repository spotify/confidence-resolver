package com.spotify.confidence.android

import android.util.Log
import com.google.protobuf.Struct
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsRequest
import com.spotify.confidence.flags.resolver.v1.ResolveWithStickyRequest
import com.spotify.confidence.flags.resolver.v1.Sdk
import com.spotify.confidence.flags.resolver.v1.SdkId
import dev.openfeature.kotlin.sdk.EvaluationContext
import dev.openfeature.kotlin.sdk.FeatureProvider
import dev.openfeature.kotlin.sdk.Hook
import dev.openfeature.kotlin.sdk.ProviderEvaluation
import dev.openfeature.kotlin.sdk.ProviderMetadata
import dev.openfeature.kotlin.sdk.Value
import dev.openfeature.kotlin.sdk.exceptions.OpenFeatureError
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.util.concurrent.atomic.AtomicReference

/**
 * OpenFeature provider for Confidence feature flags using local resolution.
 *
 * This provider evaluates feature flags locally using a WebAssembly (WASM) resolver. It
 * periodically syncs flag configurations from the Confidence service and caches them locally for
 * fast, low-latency flag evaluation.
 *
 * **Android Static Context Pattern:**
 * Unlike server-side providers where context is passed per-request, Android uses a static
 * context pattern. The evaluation context is typically set once during app initialization
 * via `OpenFeatureAPI.setEvaluationContext()` and used for all subsequent flag evaluations.
 * This provider fully supports this pattern while still allowing per-evaluation context
 * overrides when needed.
 *
 * **Usage Example:**
 * ```kotlin
 * val clientSecret = "your-application-client-secret"
 * val config = LocalProviderConfig()
 * val provider = ConfidenceLocalProvider(config, clientSecret)
 *
 * // Set global evaluation context (static context pattern)
 * OpenFeatureAPI.setEvaluationContext(
 *     ImmutableContext(
 *         targetingKey = "user-123",
 *         attributes = mapOf("country" to Value.String("US"))
 *     )
 * )
 *
 * OpenFeatureAPI.setProviderAndWait(provider)
 *
 * val client = OpenFeatureAPI.getClient()
 * val flagValue = client.getStringValue("my-flag", "default-value")
 * ```
 */
class ConfidenceLocalProvider(
    private val config: LocalProviderConfig = LocalProviderConfig(),
    private val clientSecret: String,
    private val materializationStore: MaterializationStore = UnsupportedMaterializationStore()
) : FeatureProvider {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var statePollJob: Job? = null
    private var logFlushJob: Job? = null

    private val stateFetcher = StateFetcher(clientSecret, config.httpClient ?: okhttp3.OkHttpClient())
    private val flagLogger: WasmFlagLogger = GrpcWasmFlagLogger(clientSecret, config.channelFactory)
    private lateinit var wasmResolveApi: ResolverApi

    private val resolverState = AtomicReference<ByteArray>(ByteArray(0))
    private val accountIdRef = AtomicReference<String>("")

    @Volatile
    private var initialized = false

    companion object {
        private const val TAG = "ConfidenceLocalProvider"
        internal const val PROVIDER_ID = "SDK_ID_KOTLIN_CONFIDENCE_LOCAL"

        /**
         * Creates a new ConfidenceLocalProvider with the given configuration.
         *
         * @param clientSecret The Confidence client secret
         * @param config Configuration options for the provider
         * @param materializationStore Optional custom materialization store for sticky assignments
         * @return A configured ConfidenceLocalProvider instance
         */
        @JvmStatic
        @JvmOverloads
        fun create(
            clientSecret: String,
            config: LocalProviderConfig = LocalProviderConfig(),
            materializationStore: MaterializationStore = UnsupportedMaterializationStore()
        ): ConfidenceLocalProvider {
            return ConfidenceLocalProvider(
                config = config,
                clientSecret = clientSecret,
                materializationStore = materializationStore
            )
        }
    }

    /**
     * Secondary constructor for simple initialization without custom materialization store.
     */
    constructor(clientSecret: String) : this(LocalProviderConfig(), clientSecret)

    /**
     * Secondary constructor with config and clientSecret.
     */
    constructor(config: LocalProviderConfig, clientSecret: String) : this(
        config,
        clientSecret,
        UnsupportedMaterializationStore()
    )

    override val metadata: ProviderMetadata = object : ProviderMetadata {
        override val name: String = "confidence-sdk-android-local"
    }

    override val hooks: List<Hook<*>> = emptyList()

    override suspend fun initialize(initialContext: EvaluationContext?) {
        try {
            // Fetch initial state
            stateFetcher.reload()
            resolverState.set(stateFetcher.provide())
            accountIdRef.set(stateFetcher.accountId())

            // Only initialize WASM if we got valid state
            if (accountIdRef.get().isNotEmpty()) {
                wasmResolveApi = SwapWasmResolverApi(
                    flagLogger,
                    resolverState.get(),
                    accountIdRef.get(),
                    materializationStore
                )
                initialized = true
            } else {
                Log.w(TAG, "Initial state load failed, provider starting in NOT_READY state, serving default values.")
            }

            // Start background tasks
            startBackgroundTasks()
        } catch (e: Exception) {
            Log.e(TAG, "Failed to initialize provider", e)
        }
    }

    override fun shutdown() {
        statePollJob?.cancel()
        logFlushJob?.cancel()

        if (initialized) {
            wasmResolveApi.close()
        }
    }

    /**
     * Called when the evaluation context is updated.
     * This triggers a state refresh to ensure flags are evaluated with the new context.
     */
    override suspend fun onContextSet(
        oldContext: EvaluationContext?,
        newContext: EvaluationContext
    ) {
        // Context has changed - refresh state to get updated resolutions
        Log.d(TAG, "Context updated, refreshing state...")
        try {
            stateFetcher.reload()
            resolverState.set(stateFetcher.provide())
            accountIdRef.set(stateFetcher.accountId())

            if (initialized && ::wasmResolveApi.isInitialized) {
                wasmResolveApi.updateStateAndFlushLogs(
                    resolverState.get(),
                    accountIdRef.get()
                )
            }
        } catch (e: Exception) {
            Log.w(TAG, "Failed to refresh state after context change", e)
        }
    }

    override fun getBooleanEvaluation(
        key: String,
        defaultValue: Boolean,
        context: EvaluationContext?
    ): ProviderEvaluation<Boolean> {
        return getCastedEvaluation(key, Value.Boolean(defaultValue), context) {
            (it as? Value.Boolean)?.boolean
        }
    }

    override fun getStringEvaluation(
        key: String,
        defaultValue: String,
        context: EvaluationContext?
    ): ProviderEvaluation<String> {
        return getCastedEvaluation(key, Value.String(defaultValue), context) {
            (it as? Value.String)?.string
        }
    }

    override fun getIntegerEvaluation(
        key: String,
        defaultValue: Int,
        context: EvaluationContext?
    ): ProviderEvaluation<Int> {
        return getCastedEvaluation(key, Value.Integer(defaultValue), context) {
            (it as? Value.Integer)?.integer
        }
    }

    override fun getDoubleEvaluation(
        key: String,
        defaultValue: Double,
        context: EvaluationContext?
    ): ProviderEvaluation<Double> {
        return getCastedEvaluation(key, Value.Double(defaultValue), context) {
            (it as? Value.Double)?.double
        }
    }

    override fun getObjectEvaluation(
        key: String,
        defaultValue: Value,
        context: EvaluationContext?
    ): ProviderEvaluation<Value> {
        if (!initialized) {
            return ProviderEvaluation(
                value = defaultValue,
                reason = "Provider not initialized"
            )
        }

        val flagPath = try {
            FlagPath.parse(key)
        } catch (e: IllegalArgumentException) {
            Log.w(TAG, e.message ?: "Invalid flag path")
            throw OpenFeatureError.GeneralError(e.message ?: "Invalid flag path")
        }

        val evaluationContext = TypeMapper.evaluationContextToStruct(context)

        try {
            val requestFlagName = "flags/${flagPath.flag}"

            val req = ResolveFlagsRequest.newBuilder()
                .addFlags(requestFlagName)
                .setApply(true)
                .setClientSecret(clientSecret)
                .setEvaluationContext(
                    Struct.newBuilder().putAllFields(evaluationContext.fieldsMap).build()
                )
                .setSdk(
                    Sdk.newBuilder()
                        .setId(SdkId.SDK_ID_KOTLIN_PROVIDER)
                        .setVersion(Version.VERSION)
                        .build()
                )
                .build()

            val resolveFlagResponse = wasmResolveApi
                .resolveWithSticky(
                    ResolveWithStickyRequest.newBuilder()
                        .setResolveRequest(req)
                        .setFailFastOnSticky(false)
                        .build()
                )
                .get()

            if (resolveFlagResponse.resolvedFlagsList.isEmpty()) {
                Log.w(TAG, "No active flag '${flagPath.flag}' was found")
                throw OpenFeatureError.FlagNotFoundError("No active flag '${flagPath.flag}' was found")
            }

            val responseFlagName = resolveFlagResponse.getResolvedFlags(0).flag
            if (requestFlagName != responseFlagName) {
                val unexpectedFlag = responseFlagName.removePrefix("flags/")
                Log.w(TAG, "Unexpected flag '$unexpectedFlag' from remote")
                throw OpenFeatureError.FlagNotFoundError("Unexpected flag '$unexpectedFlag' from remote")
            }

            val resolvedFlag = resolveFlagResponse.getResolvedFlags(0)

            return if (resolvedFlag.variant.isEmpty()) {
                ProviderEvaluation(
                    value = defaultValue,
                    reason = "The server returned no assignment for the flag. Typically, this happens " +
                            "if no configured rules matches the given evaluation context."
                )
            } else {
                val fullValue = TypeMapper.fromProto(resolvedFlag.value, resolvedFlag.flagSchema)

                // If a path is given, extract expected portion from the structured value
                var value = getValueForPath(flagPath.path, fullValue)

                if (value is Value.Null) {
                    value = defaultValue
                }

                ProviderEvaluation(
                    value = value,
                    reason = resolvedFlag.reason.toString(),
                    variant = resolvedFlag.variant
                )
            }
        } catch (e: OpenFeatureError) {
            throw e
        } catch (e: Exception) {
            Log.e(TAG, "Error resolving flag", e)
            throw OpenFeatureError.GeneralError("Error resolving flag: ${e.message}")
        }
    }

    private fun <T> getCastedEvaluation(
        key: String,
        wrappedDefaultValue: Value,
        context: EvaluationContext?,
        cast: (Value) -> T?
    ): ProviderEvaluation<T> {
        val objectEvaluation = getObjectEvaluation(key, wrappedDefaultValue, context)

        val castedValue = cast(objectEvaluation.value)
            ?: run {
                Log.w(TAG, "Cannot cast value '${objectEvaluation.value}' to expected type")
                throw OpenFeatureError.TypeMismatchError("Cannot cast value '${objectEvaluation.value}' to expected type")
            }

        @Suppress("UNCHECKED_CAST")
        return ProviderEvaluation(
            value = castedValue,
            variant = objectEvaluation.variant,
            reason = objectEvaluation.reason
        )
    }

    private fun startBackgroundTasks() {
        // State refresh task
        statePollJob = scope.launch {
            while (isActive) {
                delay(config.statePollInterval.toMillis())
                try {
                    stateFetcher.reload()
                    resolverState.set(stateFetcher.provide())
                    accountIdRef.set(stateFetcher.accountId())

                    if (accountIdRef.get().isNotEmpty()) {
                        if (!initialized) {
                            wasmResolveApi = SwapWasmResolverApi(
                                flagLogger,
                                resolverState.get(),
                                accountIdRef.get(),
                                materializationStore
                            )
                            initialized = true
                            Log.i(TAG, "Provider recovered and is now READY")
                        } else if (::wasmResolveApi.isInitialized) {
                            wasmResolveApi.updateStateAndFlushLogs(
                                resolverState.get(),
                                accountIdRef.get()
                            )
                        }
                    }
                } catch (e: Exception) {
                    Log.w(TAG, "Failed to refresh state", e)
                }
            }
        }

        // Log flush task
        logFlushJob = scope.launch {
            while (isActive) {
                delay(config.logFlushInterval.toMillis())
                try {
                    if (initialized && ::wasmResolveApi.isInitialized) {
                        wasmResolveApi.updateStateAndFlushLogs(
                            resolverState.get(),
                            accountIdRef.get()
                        )
                    }
                } catch (e: Exception) {
                    Log.w(TAG, "Failed to flush logs", e)
                }
            }
        }
    }

    private fun getValueForPath(path: List<String>, value: Value): Value {
        if (path.isEmpty()) {
            return value
        }

        var current = value
        for (segment in path) {
            val structure = (current as? Value.Structure)?.structure ?: return Value.Null
            current = structure[segment] ?: return Value.Null
        }
        return current
    }
}

/**
 * Represents a parsed flag path (e.g., "my-flag.nested.value").
 */
internal data class FlagPath(
    val flag: String,
    val path: List<String>
) {
    companion object {
        fun parse(key: String): FlagPath {
            if (key.isBlank()) {
                throw IllegalArgumentException("Flag key cannot be empty")
            }

            val parts = key.split(".")
            if (parts.isEmpty()) {
                throw IllegalArgumentException("Invalid flag key: $key")
            }

            return FlagPath(
                flag = parts[0],
                path = if (parts.size > 1) parts.subList(1, parts.size) else emptyList()
            )
        }
    }
}
