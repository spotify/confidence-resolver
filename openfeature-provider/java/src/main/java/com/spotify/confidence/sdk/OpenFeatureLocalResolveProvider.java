package com.spotify.confidence.sdk;

import com.google.common.annotations.VisibleForTesting;
import com.google.common.util.concurrent.ThreadFactoryBuilder;
import com.google.protobuf.Struct;
import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveWithStickyRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolvedFlag;
import com.spotify.confidence.sdk.flags.resolver.v1.Sdk;
import com.spotify.confidence.sdk.flags.resolver.v1.SdkId;
import dev.openfeature.sdk.*;
import dev.openfeature.sdk.exceptions.FlagNotFoundError;
import dev.openfeature.sdk.exceptions.GeneralError;
import dev.openfeature.sdk.exceptions.TypeMismatchError;
import io.grpc.Status;
import io.grpc.StatusRuntimeException;
import java.time.Duration;
import java.util.List;
import java.util.Optional;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Function;
import org.slf4j.Logger;

/**
 * OpenFeature provider for Confidence feature flags using local resolution.
 *
 * <p>This provider evaluates feature flags locally using a WebAssembly (WASM) resolver. It
 * periodically syncs flag configurations from the Confidence service and caches them locally for
 * fast, low-latency flag evaluation.
 *
 * <p><strong>Usage Example:</strong>
 *
 * <pre>{@code
 * String clientSecret = "your-application-client-secret";
 * LocalProviderConfig config = new LocalProviderConfig();
 * OpenFeatureLocalResolveProvider provider =
 *     new OpenFeatureLocalResolveProvider(config, clientSecret);
 *
 * OpenFeatureAPI.getInstance().setProvider(provider);
 *
 * Client client = OpenFeatureAPI.getInstance().getClient();
 * String flagValue = client.getStringValue("my-flag", "default-value");
 * }</pre>
 */
@Experimental
public class OpenFeatureLocalResolveProvider implements FeatureProvider {
  private final String clientSecret;
  private static final Logger log =
      org.slf4j.LoggerFactory.getLogger(OpenFeatureLocalResolveProvider.class);
  private final MaterializationStore materializationStore;
  private final ResolverApi wasmResolveApi;
  private static final Duration POLL_LOG_INTERVAL = Duration.ofSeconds(10);
  private static final Duration DEFAULT_POLL_INTERVAL = Duration.ofSeconds(30);
  private final ScheduledExecutorService flagsFetcherExecutor =
      Executors.newScheduledThreadPool(1, new ThreadFactoryBuilder().setDaemon(true).build());
  private final ScheduledExecutorService logPollExecutor =
      Executors.newScheduledThreadPool(1, new ThreadFactoryBuilder().setDaemon(true).build());
  private final AccountStateProvider stateProvider;
  private final AtomicReference<ProviderState> state =
      new AtomicReference<>(ProviderState.NOT_READY);
  private final ChannelFactory channelFactory;

  private static long getPollIntervalSeconds() {
    return Optional.ofNullable(System.getenv("CONFIDENCE_RESOLVER_POLL_INTERVAL_SECONDS"))
        .map(Long::parseLong)
        .orElse(DEFAULT_POLL_INTERVAL.toSeconds());
  }

  /**
   * Creates a new OpenFeature provider for local flag resolution with default configuration.
   *
   * <p>This is the simplest way to create a provider. It uses the default gRPC channel factory and
   * remote resolver fallback for sticky assignments.
   *
   * <p><strong>Example usage:</strong>
   *
   * <pre>{@code
   * OpenFeatureLocalResolveProvider provider =
   *     new OpenFeatureLocalResolveProvider("your-client-secret");
   * OpenFeatureAPI.getInstance().setProviderAndWait(provider);
   * }</pre>
   *
   * @param clientSecret the client secret for your application, used for flag resolution
   *     authentication
   */
  public OpenFeatureLocalResolveProvider(String clientSecret) {
    this(new LocalProviderConfig(), clientSecret);
  }

  /**
   * Creates a new OpenFeature provider for local flag resolution with custom channel factory.
   *
   * <p>This constructor accepts a {@link LocalProviderConfig} which allows you to customize how
   * gRPC channels are created, particularly useful for testing with mock servers or advanced
   * production scenarios requiring custom connection logic.
   *
   * <p><strong>Example with custom channel factory for testing:</strong>
   *
   * <pre>{@code
   * ChannelFactory mockFactory = (target, interceptors) ->
   *     InProcessChannelBuilder.forName("test-server")
   *         .usePlaintext()
   *         .intercept(interceptors.toArray(new ClientInterceptor[0]))
   *         .build();
   *
   * LocalProviderConfig config = new LocalProviderConfig(mockFactory);
   * OpenFeatureLocalResolveProvider provider =
   *     new OpenFeatureLocalResolveProvider(config, "client-secret");
   * }</pre>
   *
   * @param config the provider configuration including optional channel factory
   * @param clientSecret the client secret for your application, used for flag resolution
   *     authentication
   */
  public OpenFeatureLocalResolveProvider(LocalProviderConfig config, String clientSecret) {
    this(
        config,
        clientSecret,
        config.isUseRemoteMaterializationStore()
            ? new RemoteMaterializationStore(clientSecret, config.getChannelFactory())
            : new UnsupportedMaterializationStore());
  }

  /**
   * Creates a new OpenFeature provider for local flag resolution with a custom sticky resolve
   * implementation.
   *
   * <p>This constructor uses the default gRPC channel factory but allows you to provide a custom
   * {@link MaterializationStore} such as a {@link MaterializationStore} for local storage of sticky
   * assignments.
   *
   * <p><strong>Example with custom materialization repository:</strong>
   *
   * <pre>{@code
   * MaterializationStore store = new InMemoryMaterializationStoreExample();
   * OpenFeatureLocalResolveProvider provider =
   *     new OpenFeatureLocalResolveProvider("client-secret", store);
   * }</pre>
   *
   * @param clientSecret the client secret for your application, used for flag resolution
   *     authentication
   * @param materializationStore the implementation to use for handling sticky flag resolution
   */
  public OpenFeatureLocalResolveProvider(
      String clientSecret, MaterializationStore materializationStore) {
    this(new LocalProviderConfig(), clientSecret, materializationStore);
  }

  /**
   * Creates a new OpenFeature provider for local flag resolution with custom channel factory and
   * sticky resolve implementation.
   *
   * @param config the provider configuration including optional channel factory
   * @param clientSecret the client secret for your application, used for flag resolution
   *     authentication
   * @param materializationStore the implementation to use for handling sticky flag resolution
   */
  public OpenFeatureLocalResolveProvider(
      LocalProviderConfig config, String clientSecret, MaterializationStore materializationStore) {
    this.clientSecret = clientSecret;
    this.materializationStore = materializationStore;
    this.stateProvider = new FlagsAdminStateFetcher(clientSecret, config.getHttpClientFactory());
    final var wasmFlagLogger = new GrpcWasmFlagLogger(clientSecret, config.getChannelFactory());
    this.wasmResolveApi = new ThreadLocalSwapWasmResolverApi(wasmFlagLogger, materializationStore);
    this.channelFactory = config.getChannelFactory();
  }

  /**
   * Creates a new OpenFeature provider for testing with a custom WasmFlagLogger.
   *
   * <p>This constructor allows injecting a custom WasmFlagLogger for testing purposes, enabling
   * verification of flag log data without making network calls.
   *
   * @param accountStateProvider the state provider for resolver state
   * @param clientSecret the client secret for authentication
   * @param materializationStore the implementation for sticky flag resolution
   * @param wasmFlagLogger the flag logger to use (e.g., CapturingWasmFlagLogger for testing)
   */
  @VisibleForTesting
  public OpenFeatureLocalResolveProvider(
      AccountStateProvider accountStateProvider,
      String clientSecret,
      MaterializationStore materializationStore,
      WasmFlagLogger wasmFlagLogger) {
    this.materializationStore = materializationStore;
    this.clientSecret = clientSecret;
    this.stateProvider = accountStateProvider;
    this.wasmResolveApi = new ThreadLocalSwapWasmResolverApi(wasmFlagLogger, materializationStore);
    this.channelFactory = new LocalProviderConfig().getChannelFactory();
  }

  @Override
  public ProviderState getState() {
    return state.get();
  }

  @Override
  public void initialize(EvaluationContext evaluationContext) {
    stateProvider.reload();
    final AtomicReference<byte[]> resolverStateProtobuf =
        new AtomicReference<>(stateProvider.provide());
    final AtomicReference<String> accountIdRef = new AtomicReference<>(stateProvider.accountId());

    // Only initialize WASM and set READY if we got valid state (non-empty accountId)
    if (!accountIdRef.get().isEmpty()) {
      wasmResolveApi.init(resolverStateProtobuf.get(), accountIdRef.get());
      this.state.set(ProviderState.READY);
    } else {
      log.warn(
          "Initial state load failed, provider starting in NOT_READY state, serving default values.");
    }

    final long pollIntervalSeconds = getPollIntervalSeconds();
    scheduleStateRefresh(resolverStateProtobuf, accountIdRef, pollIntervalSeconds);

    logPollExecutor.scheduleAtFixedRate(
        () -> {
          if (wasmResolveApi.isInitialized()) {
            wasmResolveApi.updateStateAndFlushLogs(resolverStateProtobuf.get(), accountIdRef.get());
          }
        },
        POLL_LOG_INTERVAL.getSeconds(),
        POLL_LOG_INTERVAL.getSeconds(),
        TimeUnit.SECONDS);
  }

  private void scheduleStateRefresh(
      AtomicReference<byte[]> resolverStateProtobuf,
      AtomicReference<String> accountIdRef,
      long pollIntervalSeconds) {
    if (flagsFetcherExecutor.isShutdown()) {
      return;
    }

    // Use short retry interval (1s) when not initialized, normal interval otherwise
    long delaySeconds = wasmResolveApi.isInitialized() ? pollIntervalSeconds : 1;

    flagsFetcherExecutor.schedule(
        () -> {
          stateProvider.reload();
          resolverStateProtobuf.set(stateProvider.provide());
          accountIdRef.set(stateProvider.accountId());

          if (!accountIdRef.get().isEmpty() && !wasmResolveApi.isInitialized()) {
            wasmResolveApi.init(resolverStateProtobuf.get(), accountIdRef.get());
            this.state.set(ProviderState.READY);
            log.info("Provider recovered and is now READY");
          }

          scheduleStateRefresh(resolverStateProtobuf, accountIdRef, pollIntervalSeconds);
        },
        delaySeconds,
        TimeUnit.SECONDS);
  }

  @Override
  public Metadata getMetadata() {
    return () -> "confidence-sdk-java-local";
  }

  @Override
  public ProviderEvaluation<Boolean> getBooleanEvaluation(
      String key, Boolean defaultValue, EvaluationContext ctx) {
    return getCastedEvaluation(key, defaultValue, ctx, Value::asBoolean);
  }

  @Override
  public ProviderEvaluation<String> getStringEvaluation(
      String key, String defaultValue, EvaluationContext ctx) {
    return getCastedEvaluation(key, defaultValue, ctx, Value::asString);
  }

  @Override
  public ProviderEvaluation<Integer> getIntegerEvaluation(
      String key, Integer defaultValue, EvaluationContext ctx) {
    return getCastedEvaluation(key, defaultValue, ctx, Value::asInteger);
  }

  @Override
  public ProviderEvaluation<Double> getDoubleEvaluation(
      String key, Double defaultValue, EvaluationContext ctx) {
    return getCastedEvaluation(key, defaultValue, ctx, Value::asDouble);
  }

  private <T> ProviderEvaluation<T> getCastedEvaluation(
      String key, T defaultValue, EvaluationContext ctx, Function<Value, T> cast) {
    final Value wrappedDefaultValue;
    try {
      wrappedDefaultValue = new Value(defaultValue);
    } catch (InstantiationException e) {
      // this is not going to happen because we only call the constructor with supported types
      throw new RuntimeException(e);
    }

    final ProviderEvaluation<Value> objectEvaluation =
        getObjectEvaluation(key, wrappedDefaultValue, ctx);

    final T castedValue = cast.apply(objectEvaluation.getValue());
    if (castedValue == null) {
      log.warn("Cannot cast value '{}' to expected type", objectEvaluation.getValue().toString());
      throw new TypeMismatchError(
          String.format("Cannot cast value '%s' to expected type", objectEvaluation.getValue()));
    }

    final ProviderEvaluation<T> result =
        ProviderEvaluation.<T>builder()
            .value(castedValue)
            .variant(objectEvaluation.getVariant())
            .reason(objectEvaluation.getReason())
            .errorMessage(objectEvaluation.getErrorMessage())
            .errorCode(objectEvaluation.getErrorCode())
            .build();

    if (result.getErrorCode() != null) {
      log.warn(
          "Flag evaluation for '{}' returned error code: {}, message: {}",
          key,
          result.getErrorCode(),
          result.getErrorMessage());
    }

    return result;
  }

  @Override
  public void shutdown() {
    state.set(ProviderState.NOT_READY);
    log.debug("Shutting down scheduled executors");
    flagsFetcherExecutor.shutdown();
    logPollExecutor.shutdown();

    try {
      if (!flagsFetcherExecutor.awaitTermination(1, TimeUnit.SECONDS)) {
        log.warn("Flags fetcher executor did not terminate gracefully");
        flagsFetcherExecutor.shutdownNow();
      }
      if (!logPollExecutor.awaitTermination(1, TimeUnit.SECONDS)) {
        log.warn("Log poll executor did not terminate gracefully");
        logPollExecutor.shutdownNow();
      }
    } catch (InterruptedException e) {
      log.warn("Interrupted while waiting for scheduled executors to shut down", e);
      flagsFetcherExecutor.shutdownNow();
      logPollExecutor.shutdownNow();
      Thread.currentThread().interrupt();
    }

    // if we created the materialization store ourselves we are responsible for shutting it down
    if (materializationStore instanceof RemoteMaterializationStore remoteMaterializationStore) {
      remoteMaterializationStore.shutdown();
    }

    // wasmResolveApi.close() flushes logs and calls flagLogger.shutdown() which waits for pending
    // writes
    this.wasmResolveApi.close();

    FeatureProvider.super.shutdown();
  }

  @Override
  public ProviderEvaluation<Value> getObjectEvaluation(
      String key, Value defaultValue, EvaluationContext ctx) {

    final FlagPath flagPath;
    try {
      flagPath = FlagPath.getPath(key);
    } catch (Exceptions.IllegalValuePath e) {
      log.warn(e.getMessage());
      throw new RuntimeException(e);
    }

    final Struct evaluationContext = OpenFeatureUtils.convertToProto(ctx);
    // resolve the flag by calling the resolver API
    ResolveFlagsResponse resolveFlagResponse;
    try {
      final String requestFlagName = "flags/" + flagPath.getFlag();

      final var req =
          ResolveFlagsRequest.newBuilder()
              .addFlags(requestFlagName)
              .setApply(true)
              .setClientSecret(clientSecret)
              .setEvaluationContext(
                  Struct.newBuilder().putAllFields(evaluationContext.getFieldsMap()).build())
              .setSdk(
                  Sdk.newBuilder()
                      .setId(SdkId.SDK_ID_JAVA_LOCAL_PROVIDER)
                      .setVersion(Version.VERSION)
                      .build())
              .build();

      resolveFlagResponse =
          wasmResolveApi
              .resolveWithSticky(
                  ResolveWithStickyRequest.newBuilder()
                      .setResolveRequest(req)
                      .setFailFastOnSticky(false)
                      .build())
              .toCompletableFuture()
              .get();

      if (resolveFlagResponse.getResolvedFlagsList().isEmpty()) {
        log.warn("No active flag '{}' was found", flagPath.getFlag());
        throw new FlagNotFoundError(
            String.format("No active flag '%s' was found", flagPath.getFlag()));
      }

      final String responseFlagName = resolveFlagResponse.getResolvedFlags(0).getFlag();
      if (!requestFlagName.equals(responseFlagName)) {
        log.warn("Unexpected flag '{}' from remote", responseFlagName.replaceFirst("^flags/", ""));
        throw new FlagNotFoundError(
            String.format(
                "Unexpected flag '%s' from remote", responseFlagName.replaceFirst("^flags/", "")));
      }

      final ResolvedFlag resolvedFlag = resolveFlagResponse.getResolvedFlags(0);

      if (resolvedFlag.getVariant().isEmpty()) {
        return ProviderEvaluation.<Value>builder()
            .value(defaultValue)
            .reason(
                "The server returned no assignment for the flag. Typically, this happens "
                    + "if no configured rules matches the given evaluation context.")
            .build();
      } else {
        final Value fullValue =
            OpenFeatureTypeMapper.from(resolvedFlag.getValue(), resolvedFlag.getFlagSchema());

        // if a path is given, extract expected portion from the structured value
        Value value = OpenFeatureUtils.getValueForPath(flagPath.getPath(), fullValue);

        if (value.isNull()) {
          value = defaultValue;
        }

        // regular resolve was successful
        return ProviderEvaluation.<Value>builder()
            .value(value)
            .reason(resolvedFlag.getReason().toString())
            .variant(resolvedFlag.getVariant())
            .build();
      }
    } catch (StatusRuntimeException e) {
      handleStatusRuntimeException(e);
      throw new GeneralError("Unknown error occurred when calling the provider backend");
    } catch (ExecutionException | InterruptedException e) {
      throw new RuntimeException(e);
    }
  }

  /**
   * Resolves multiple flags at once for the given evaluation context. This method is intended for
   * use by {@link FlagResolverService} to proxy resolve requests from client SDKs.
   *
   * @param ctx the evaluation context containing targeting key and other attributes
   * @param flagNames the names of flags to resolve (without "flags/" prefix)
   * @param apply whether to mark the flags as applied immediately
   * @return the resolve response containing all resolved flags
   */
  ResolveFlagsResponse resolve(EvaluationContext ctx, List<String> flagNames, boolean apply) {
    final Struct evaluationContext = OpenFeatureUtils.convertToProto(ctx);

    final var reqBuilder =
        ResolveFlagsRequest.newBuilder()
            .setApply(apply)
            .setClientSecret(clientSecret)
            .setEvaluationContext(
                Struct.newBuilder().putAllFields(evaluationContext.getFieldsMap()).build())
            .setSdk(
                Sdk.newBuilder()
                    .setId(SdkId.SDK_ID_JAVA_LOCAL_PROVIDER)
                    .setVersion(Version.VERSION)
                    .build());

    // Add flags with proper prefix
    for (String flagName : flagNames) {
      if (flagName.startsWith("flags/")) {
        reqBuilder.addFlags(flagName);
      } else {
        reqBuilder.addFlags("flags/" + flagName);
      }
    }

    try {
      return wasmResolveApi
          .resolveWithSticky(
              ResolveWithStickyRequest.newBuilder()
                  .setResolveRequest(reqBuilder.build())
                  .setFailFastOnSticky(false)
                  .build())
          .toCompletableFuture()
          .get();
    } catch (ExecutionException | InterruptedException e) {
      throw new RuntimeException("Failed to resolve flags", e);
    }
  }

  /**
   * Applies flags that were previously resolved with apply=false. This method is intended for use
   * by {@link FlagResolverService} to proxy apply requests from client SDKs.
   *
   * @param request the apply flags request containing resolve token and flags to apply
   */
  void applyFlags(ApplyFlagsRequest request) {
    wasmResolveApi.applyFlags(request);
  }

  private static void handleStatusRuntimeException(StatusRuntimeException e) {
    if (e.getStatus().getCode() == Status.Code.DEADLINE_EXCEEDED) {
      log.error("Deadline exceeded when calling provider backend", e);
      throw new GeneralError("Deadline exceeded when calling provider backend");
    } else if (e.getStatus().getCode() == Status.Code.UNAVAILABLE) {
      log.error("Provider backend is unavailable", e);
      throw new GeneralError("Provider backend is unavailable");
    } else if (e.getStatus().getCode() == Status.Code.UNAUTHENTICATED) {
      log.error("UNAUTHENTICATED", e);
      throw new GeneralError("UNAUTHENTICATED");
    } else {
      log.error(
          "Unknown error occurred when calling the provider backend. Grpc status code {}",
          e.getStatus().getCode(),
          e);
      throw new GeneralError(
          String.format(
              "Unknown error occurred when calling the provider backend. Exception: %s",
              e.getMessage()));
    }
  }
}
