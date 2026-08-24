package com.spotify.confidence.sdk;

import com.google.common.annotations.VisibleForTesting;
import com.google.common.util.concurrent.ThreadFactoryBuilder;
import com.google.protobuf.Struct;
import com.google.protobuf.Timestamp;
import com.spotify.confidence.sdk.events.v1.EventError;
import com.spotify.confidence.sdk.events.v1.EventsServiceGrpc;
import com.spotify.confidence.sdk.events.v1.PublishEventsRequest;
import com.spotify.confidence.sdk.events.v1.PublishEventsResponse;
import com.spotify.confidence.sdk.events.wasm.v1.FlushEventsResponse;
import com.spotify.confidence.sdk.events.wasm.v1.TrackEventRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.RegisterResolveRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveReason;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolvedFlag;
import com.spotify.confidence.sdk.flags.resolver.v1.Sdk;
import com.spotify.confidence.sdk.flags.resolver.v1.SdkId;
import dev.openfeature.sdk.*;
import dev.openfeature.sdk.exceptions.FlagNotFoundError;
import dev.openfeature.sdk.exceptions.GeneralError;
import dev.openfeature.sdk.exceptions.TypeMismatchError;
import io.grpc.ManagedChannel;
import io.grpc.Status;
import io.grpc.StatusRuntimeException;
import java.time.Duration;
import java.time.Instant;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicLong;
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
  private final LocalResolver resolver;
  private final WasmFlagLogger flagLogger;
  private final MaterializationStore materializationStore;
  private final boolean disableExposureCollection;
  private static final Duration ASSIGN_LOG_FLUSH_INTERVAL = Duration.ofMillis(100);
  private static final Duration DEFAULT_POLL_INTERVAL = Duration.ofSeconds(15);
  private static final Duration EVENT_FLUSH_INTERVAL = Duration.ofSeconds(15);
  private static final Duration SHUTDOWN_GRACE = Duration.ofSeconds(5);
  private static final int MAX_DRAIN_BATCHES = 100;

  /**
   * Number of event publish attempts between failure-rate log lines. Mirrors {@code
   * GrpcWasmFlagLogger.STATS_WINDOW}: publish failures are swallowed per batch so that a broken
   * events backend cannot take down flag resolution, and this window is the only signal that they
   * are happening.
   */
  private static final int EVENT_STATS_WINDOW = 10;

  private final AtomicLong eventPublishAttempts = new AtomicLong();
  private final AtomicLong eventPublishFailures = new AtomicLong();
  private final ScheduledExecutorService flagsFetcherExecutor = newFlagsFetcherExecutor();
  private final ScheduledExecutorService assignLogExecutor =
      Executors.newScheduledThreadPool(1, new ThreadFactoryBuilder().setDaemon(true).build());
  private final AccountStateProvider stateProvider;
  private final AtomicReference<ProviderState> state =
      new AtomicReference<>(ProviderState.NOT_READY);
  private volatile boolean initialized = false;
  private volatile byte[] lastStateBytes = null;
  @VisibleForTesting boolean forcedFetcherShutdown = false;
  private static final Sdk SDK =
      Sdk.newBuilder().setId(SdkId.SDK_ID_JAVA_LOCAL_PROVIDER).setVersion(Version.VERSION).build();

  /**
   * SDK identity reported to the events service. This is a different {@code Sdk}/{@code SdkId} pair
   * from the flag-resolver one above — same variant name, different proto package.
   */
  private static final com.spotify.confidence.sdk.events.v1.Sdk EVENTS_SDK =
      com.spotify.confidence.sdk.events.v1.Sdk.newBuilder()
          .setId(com.spotify.confidence.sdk.events.v1.SdkId.SDK_ID_JAVA_LOCAL_PROVIDER)
          .setVersion(Version.VERSION)
          .build();

  // Event tracking (optional — null when not configured)
  private final WasmEventResolver eventResolver;
  private final ScheduledExecutorService eventFlushExecutor;
  private final ManagedChannel eventsChannel;
  private final EventsServiceGrpc.EventsServiceBlockingStub eventsStub;

  private static ScheduledExecutorService newFlagsFetcherExecutor() {
    final ScheduledThreadPoolExecutor executor =
        new ScheduledThreadPoolExecutor(1, new ThreadFactoryBuilder().setDaemon(true).build());
    // The poll task reschedules itself, so there is always one pending. Drop it on shutdown instead
    // of letting awaitTermination wait for a poll that won't fire for another poll interval.
    executor.setExecuteExistingDelayedTasksAfterShutdownPolicy(false);
    return executor;
  }

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
    this.disableExposureCollection = config.isDisableExposureCollection();
    if (config.getEncryptionKey() == null) {
      log.warn(
          "No encryptionKey provided. Falling back to unencrypted state."
              + " An encryption key will be required in an upcoming version.");
    }
    this.stateProvider =
        new FlagsAdminStateFetcher(
            clientSecret, config.getHttpClientFactory(), config.getEncryptionKey());
    final var wasmFlagLogger =
        new GrpcWasmFlagLogger(
            clientSecret, config.getChannelFactory(), config.getHttpClientFactory());
    this.flagLogger = wasmFlagLogger;
    final Map<String, String> initLabels =
        Map.of("encryption", String.valueOf(config.getEncryptionKey() != null));
    final int numInstances = PooledResolver.getNumInstances(config.getResolverPoolSize());
    final LocalResolver telemetryResolver =
        new ProviderTelemetryResolver(
            flagLogger::write,
            SDK,
            initLabels,
            providerLogSink ->
                new PooledResolver(
                    numInstances,
                    () ->
                        new RecoveringResolver(
                            () ->
                                new WasmLocalResolver(
                                    providerLogSink,
                                    config.isEnableApplyDedup(),
                                    config.isDisableExposureCollection()))));
    this.resolver = new MaterializingResolver(telemetryResolver, materializationStore);

    // Initialize event tracking if event WASM is provided
    if (config.getEventWasmBytes() != null) {
      this.eventResolver = new WasmEventResolver(config.getEventWasmBytes());
      this.eventFlushExecutor =
          Executors.newScheduledThreadPool(
              1,
              new ThreadFactoryBuilder().setDaemon(true).setNameFormat("event-flush-%d").build());
      this.eventsChannel = GrpcUtil.createConfidenceEventsChannel(config.getChannelFactory());
      this.eventsStub = EventsServiceGrpc.newBlockingStub(this.eventsChannel);
    } else {
      this.eventResolver = null;
      this.eventFlushExecutor = null;
      this.eventsChannel = null;
      this.eventsStub = null;
    }
  }

  /**
   * Creates a new OpenFeature provider for testing with a custom WasmFlagLogger.
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
    this(accountStateProvider, clientSecret, materializationStore, wasmFlagLogger, false);
  }

  @VisibleForTesting
  public OpenFeatureLocalResolveProvider(
      AccountStateProvider accountStateProvider,
      String clientSecret,
      MaterializationStore materializationStore,
      WasmFlagLogger wasmFlagLogger,
      boolean enableApplyDedup) {
    this(
        accountStateProvider,
        clientSecret,
        materializationStore,
        wasmFlagLogger,
        enableApplyDedup,
        false);
  }

  @VisibleForTesting
  public OpenFeatureLocalResolveProvider(
      AccountStateProvider accountStateProvider,
      String clientSecret,
      MaterializationStore materializationStore,
      WasmFlagLogger wasmFlagLogger,
      boolean enableApplyDedup,
      boolean disableExposureCollection) {
    this.clientSecret = clientSecret;
    this.materializationStore = materializationStore;
    this.disableExposureCollection = disableExposureCollection;
    this.stateProvider = accountStateProvider;
    this.flagLogger = wasmFlagLogger;
    final int numInstances =
        PooledResolver.getNumInstances(LocalProviderConfig.DEFAULT_RESOLVER_POOL_SIZE);
    final LocalResolver telemetryResolver =
        new ProviderTelemetryResolver(
            wasmFlagLogger::write,
            SDK,
            Map.of(),
            providerLogSink ->
                new PooledResolver(
                    numInstances,
                    () ->
                        new RecoveringResolver(
                            () ->
                                new WasmLocalResolver(
                                    providerLogSink,
                                    enableApplyDedup,
                                    disableExposureCollection))));
    this.resolver = new MaterializingResolver(telemetryResolver, materializationStore);
    this.eventResolver = null;
    this.eventFlushExecutor = null;
    this.eventsChannel = null;
    this.eventsStub = null;
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
      resolver.setResolverState(resolverStateProtobuf.get(), accountIdRef.get(), SDK);
      flagLogger.updateLogRouting(stateProvider.logDestinations(), accountIdRef.get());
      initialized = true;
      this.state.set(ProviderState.READY);
    } else {
      log.warn(
          "Initial state load failed, provider starting in NOT_READY state, serving default"
              + " values.");
    }

    final long pollIntervalSeconds = getPollIntervalSeconds();
    scheduleStateRefresh(resolverStateProtobuf, accountIdRef, pollIntervalSeconds);

    // Assign flush only. Resolve logs and telemetry still go out via
    // flushAllLogs() on the state-refresh cycle and resolver.close() on shutdown.
    if (!disableExposureCollection) {
      assignLogExecutor.scheduleAtFixedRate(
          () -> {
            try {
              if (initialized) {
                resolver.flushAssignLogs();
              }
            } catch (RuntimeException e) {
              log.error("Failed to flush assign logs", e);
            }
          },
          ASSIGN_LOG_FLUSH_INTERVAL.toMillis(),
          ASSIGN_LOG_FLUSH_INTERVAL.toMillis(),
          TimeUnit.MILLISECONDS);
    }

    // Schedule event flushing if event tracking is enabled
    if (eventFlushExecutor != null && eventResolver != null) {
      eventFlushExecutor.scheduleAtFixedRate(
          this::doFlushAndSendEvents,
          EVENT_FLUSH_INTERVAL.toMillis(),
          EVENT_FLUSH_INTERVAL.toMillis(),
          TimeUnit.MILLISECONDS);
    }
  }

  private void scheduleStateRefresh(
      AtomicReference<byte[]> resolverStateProtobuf,
      AtomicReference<String> accountIdRef,
      long pollIntervalSeconds) {
    if (flagsFetcherExecutor.isShutdown()) {
      return;
    }
    // Use short retry interval (1s) when not initialized, normal interval otherwise
    long delaySeconds = initialized ? pollIntervalSeconds : 1;

    flagsFetcherExecutor.schedule(
        () -> {
          try {
            stateProvider.reload();
            resolverStateProtobuf.set(stateProvider.provide());
            accountIdRef.set(stateProvider.accountId());

            if (!accountIdRef.get().isEmpty()) {
              // Always update log routing — destinations can change independently of state bytes
              flagLogger.updateLogRouting(stateProvider.logDestinations(), accountIdRef.get());

              if (!initialized) {
                resolver.setResolverState(resolverStateProtobuf.get(), accountIdRef.get(), SDK);
                lastStateBytes = resolverStateProtobuf.get();
                initialized = true;
                this.state.set(ProviderState.READY);
                log.info("Provider recovered and is now READY");
              } else {
                // Flush logs before state update to reduce WASM heap fragmentation (#455)
                resolver.flushAllLogs();

                // Only push state into the wasm instances when it actually changed — the wasm
                // execution inside setResolverState is expensive (runs across all pool slots).
                final byte[] newState = resolverStateProtobuf.get();
                if (!java.util.Arrays.equals(newState, lastStateBytes)) {
                  resolver.setResolverState(newState, accountIdRef.get(), SDK);
                  lastStateBytes = newState;
                }
              }
            }
          } catch (RuntimeException e) {
            log.error("State refresh failed", e);
          } finally {
            scheduleStateRefresh(resolverStateProtobuf, accountIdRef, pollIntervalSeconds);
          }
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
    final long startNanos = System.nanoTime();
    final Value wrappedDefaultValue;
    try {
      wrappedDefaultValue = new Value(defaultValue);
    } catch (InstantiationException e) {
      throw new RuntimeException(e);
    }

    final ProviderEvaluation<Value> objectEvaluation =
        getObjectEvaluationInternal(key, wrappedDefaultValue, ctx, startNanos);

    final T castedValue = cast.apply(objectEvaluation.getValue());
    if (castedValue == null) {
      log.warn("Cannot cast value '{}' to expected type", objectEvaluation.getValue().toString());
      doRegisterResolve(ResolveReason.RESOLVE_REASON_TYPE_MISMATCH, startNanos);
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
    assignLogExecutor.shutdown();
    if (eventFlushExecutor != null) {
      eventFlushExecutor.shutdown();
    }

    final long graceSeconds = SHUTDOWN_GRACE.toSeconds();
    try {
      if (!flagsFetcherExecutor.awaitTermination(graceSeconds, TimeUnit.SECONDS)) {
        log.warn(
            "Flags fetcher executor did not terminate within {}s, forcing shutdown", graceSeconds);
        forcedFetcherShutdown = true;
        flagsFetcherExecutor.shutdownNow();
      }
      if (!assignLogExecutor.awaitTermination(graceSeconds, TimeUnit.SECONDS)) {
        log.warn(
            "Assign log executor did not terminate within {}s, forcing shutdown", graceSeconds);
        assignLogExecutor.shutdownNow();
      }
      if (eventFlushExecutor != null
          && !eventFlushExecutor.awaitTermination(graceSeconds, TimeUnit.SECONDS)) {
        log.warn(
            "Event flush executor did not terminate within {}s, forcing shutdown", graceSeconds);
        eventFlushExecutor.shutdownNow();
      }
    } catch (InterruptedException e) {
      log.warn("Interrupted while waiting for scheduled executors to shut down", e);
      flagsFetcherExecutor.shutdownNow();
      assignLogExecutor.shutdownNow();
      if (eventFlushExecutor != null) {
        eventFlushExecutor.shutdownNow();
      }
      Thread.currentThread().interrupt();
    }

    // Drain remaining events before closing the event resolver
    drainEvents();
    if (eventResolver != null) {
      eventResolver.close();
    }
    if (eventsChannel != null) {
      eventsChannel.shutdown();
      try {
        if (!eventsChannel.awaitTermination(graceSeconds, TimeUnit.SECONDS)) {
          eventsChannel.shutdownNow();
        }
      } catch (InterruptedException e) {
        eventsChannel.shutdownNow();
        Thread.currentThread().interrupt();
      }
    }

    // if we created the materialization store ourselves we are responsible for shutting it down
    if (materializationStore instanceof RemoteMaterializationStore remoteMaterializationStore) {
      remoteMaterializationStore.shutdown();
    }

    // resolver.close() flushes remaining logs via the log sink
    this.resolver.close();

    // flagLogger.shutdown() waits for pending async writes to complete
    this.flagLogger.shutdown();

    FeatureProvider.super.shutdown();
  }

  @Override
  public ProviderEvaluation<Value> getObjectEvaluation(
      String key, Value defaultValue, EvaluationContext ctx) {
    return getObjectEvaluationInternal(key, defaultValue, ctx, System.nanoTime());
  }

  private ProviderEvaluation<Value> getObjectEvaluationInternal(
      String key, Value defaultValue, EvaluationContext ctx, long startNanos) {

    final FlagPath flagPath;
    try {
      flagPath = FlagPath.getPath(key);
    } catch (Exceptions.IllegalValuePath e) {
      log.warn(e.getMessage());
      throw new RuntimeException(e);
    }

    // apply=false covers both provider disableExposureCollection and per-eval
    // `_confidence_skip_apply`. Provider disableExposureCollection is also set on the WASM
    // guest via setResolverState so assign/token are skipped entirely;
    // apply=false alone would still mint a deferred token.
    final boolean disableExposureCollection =
        this.disableExposureCollection || OpenFeatureUtils.isSkipApply(ctx);
    final Struct evaluationContext = OpenFeatureUtils.convertToProto(ctx);
    ResolveFlagsResponse resolveFlagResponse;
    try {
      final String requestFlagName = "flags/" + flagPath.getFlag();

      final var req =
          ResolveFlagsRequest.newBuilder()
              .addFlags(requestFlagName)
              .setApply(!disableExposureCollection)
              .setClientSecret(clientSecret)
              .setEvaluationContext(
                  Struct.newBuilder().putAllFields(evaluationContext.getFieldsMap()).build())
              .setSdk(
                  Sdk.newBuilder()
                      .setId(SdkId.SDK_ID_JAVA_LOCAL_PROVIDER)
                      .setVersion(Version.VERSION)
                      .build())
              .build();

      final var processResponse =
          resolver
              .resolveProcess(
                  ResolveProcessRequest.newBuilder().setWithoutMaterializations(req).build())
              .toCompletableFuture()
              .join();
      resolveFlagResponse = processResponse.getResolved().getResponse();

      if (resolveFlagResponse.getResolvedFlagsList().isEmpty()) {
        log.warn("No active flag '{}' was found", flagPath.getFlag());
        doRegisterResolve(ResolveReason.RESOLVE_REASON_FLAG_NOT_FOUND, startNanos);
        throw new FlagNotFoundError(
            String.format("No active flag '%s' was found", flagPath.getFlag()));
      }

      final String responseFlagName = resolveFlagResponse.getResolvedFlags(0).getFlag();
      if (!requestFlagName.equals(responseFlagName)) {
        log.warn("Unexpected flag '{}' from remote", responseFlagName.replaceFirst("^flags/", ""));
        doRegisterResolve(ResolveReason.RESOLVE_REASON_FLAG_NOT_FOUND, startNanos);
        throw new FlagNotFoundError(
            String.format(
                "Unexpected flag '%s' from remote", responseFlagName.replaceFirst("^flags/", "")));
      }

      final ResolvedFlag resolvedFlag = resolveFlagResponse.getResolvedFlags(0);

      if (resolvedFlag.getVariant().isEmpty()) {
        doRegisterResolve(resolvedFlag.getReason(), startNanos);
        return ProviderEvaluation.<Value>builder()
            .value(defaultValue)
            .reason(
                "The server returned no assignment for the flag. Typically, this happens "
                    + "if no configured rules matches the given evaluation context.")
            .build();
      } else {
        final Value fullValue =
            OpenFeatureTypeMapper.from(resolvedFlag.getValue(), resolvedFlag.getFlagSchema());

        Value value = OpenFeatureUtils.getValueForPath(flagPath.getPath(), fullValue);

        if (value.isNull()) {
          value = defaultValue;
        }

        doRegisterResolve(resolvedFlag.getReason(), startNanos);
        return ProviderEvaluation.<Value>builder()
            .value(value)
            .reason(resolvedFlag.getReason().toString())
            .variant(resolvedFlag.getVariant())
            .build();
      }
    } catch (CompletionException e) {
      if (e.getCause() instanceof MaterializationNotSupportedException) {
        log.warn(
            "Flag '{}' requires materializations but no materialization store is configured. "
                + "Enable it via LocalProviderConfig.builder().useRemoteMaterializationStore(true)",
            flagPath.getFlag());
        doRegisterResolve(ResolveReason.RESOLVE_REASON_MATERIALIZATION_NOT_SUPPORTED, startNanos);
        return ProviderEvaluation.<Value>builder()
            .value(defaultValue)
            .reason(ResolveReason.RESOLVE_REASON_MATERIALIZATION_NOT_SUPPORTED.toString())
            .build();
      }
      throw e;
    } catch (StatusRuntimeException e) {
      handleStatusRuntimeException(e);
      throw new GeneralError("Unknown error occurred when calling the provider backend");
    }
  }

  // ── Event Tracking ─────────────────────────────────────────────────────────

  /**
   * Tracks an event through the Confidence event engine. Events are buffered in the WASM engine and
   * periodically flushed to the Confidence events API.
   *
   * <p>This method is a no-op if event tracking was not configured (i.e., no event WASM binary was
   * provided in {@link LocalProviderConfig}).
   *
   * @param trackingEventName the event name (e.g., "purchase_completed")
   * @param context the OpenFeature evaluation context
   * @param details tracking event details including an optional numeric value and custom data
   */
  @Override
  public void track(
      String trackingEventName, EvaluationContext context, TrackingEventDetails details) {
    if (eventResolver == null) {
      return;
    }
    try {
      final Instant now = Instant.now();
      final TrackEventRequest.Builder reqBuilder =
          TrackEventRequest.newBuilder()
              .setEventName(trackingEventName)
              .setEventTime(
                  Timestamp.newBuilder()
                      .setSeconds(now.getEpochSecond())
                      .setNanos(now.getNano())
                      .build());

      if (context != null) {
        reqBuilder.setContext(OpenFeatureUtils.convertToProto(context));
      }

      if (details != null) {
        details.getValue().ifPresent(v -> reqBuilder.setValue(v.doubleValue()));
        // Convert custom data fields from TrackingEventDetails (which extends Structure)
        if (!details.isEmpty()) {
          final Struct.Builder dataBuilder = Struct.newBuilder();
          details
              .asMap()
              .forEach(
                  (key, value) -> dataBuilder.putFields(key, OpenFeatureTypeMapper.from(value)));
          reqBuilder.setData(dataBuilder.build());
        }
      }

      eventResolver.trackEvent(reqBuilder.build());
    } catch (RuntimeException e) {
      log.warn("Failed to track event '{}'", trackingEventName, e);
    }
  }

  /**
   * Flushes buffered events from the WASM engine and publishes them to the Confidence events
   * service.
   */
  private void doFlushAndSendEvents() {
    if (eventResolver == null) {
      return;
    }
    try {
      final FlushEventsResponse batch = eventResolver.flushEvents();
      if (batch.getEventsCount() > 0) {
        sendEvents(batch);
      }
    } catch (RuntimeException e) {
      log.warn("Failed to flush events", e);
    }
  }

  /**
   * Drains all remaining events from the WASM engine by calling flush in a loop until no events
   * remain.
   */
  private void drainEvents() {
    if (eventResolver == null) {
      return;
    }
    try {
      // Bounded: sendEvents swallows network failures, so an unbounded loop would
      // spin forever if the events API is unreachable during shutdown.
      for (int i = 0; i < MAX_DRAIN_BATCHES; i++) {
        final FlushEventsResponse batch = eventResolver.flushEvents();
        if (batch.getEventsCount() == 0) {
          return;
        }
        sendEvents(batch);
      }
      log.warn(
          "Event drain hit the {}-batch limit on shutdown; dropping the rest", MAX_DRAIN_BATCHES);
    } catch (RuntimeException e) {
      log.warn("Failed to drain events on shutdown", e);
    }
  }

  /** Publishes a batch of events to the Confidence events service over gRPC. */
  private void sendEvents(FlushEventsResponse batch) {
    if (eventsStub == null) {
      return;
    }
    final Instant now = Instant.now();
    final PublishEventsRequest request =
        PublishEventsRequest.newBuilder()
            .setClientSecret(clientSecret)
            .addAllEvents(batch.getEventsList())
            .setSendTime(
                Timestamp.newBuilder().setSeconds(now.getEpochSecond()).setNanos(now.getNano()))
            .setSdk(EVENTS_SDK)
            .build();
    try {
      final PublishEventsResponse response = eventsStub.publishEvents(request);
      for (final EventError error : response.getErrorsList()) {
        log.error(
            "Failed to publish event at index {}: {} {}",
            error.getIndex(),
            error.getReason(),
            error.getMessage());
      }
    } catch (StatusRuntimeException e) {
      eventPublishFailures.incrementAndGet();
      log.warn("Failed to send events", e);
    }
    if (eventPublishAttempts.incrementAndGet() % EVENT_STATS_WINDOW == 0) {
      final long failCount = eventPublishFailures.getAndSet(0);
      if (failCount > 0) {
        log.warn("Event publish failures: {}/{}", failCount, EVENT_STATS_WINDOW);
      }
    }
  }

  private void doRegisterResolve(ResolveReason reason, long startNanos) {
    long latencyUs = (System.nanoTime() - startNanos) / 1000;
    try {
      resolver.registerResolve(
          RegisterResolveRequest.newBuilder()
              .setReason(reason)
              .setLatencyUs((int) Math.min(latencyUs, Integer.MAX_VALUE))
              .build());
    } catch (Exception e) {
      log.warn("Failed to register resolve telemetry", e);
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
  CompletionStage<ResolveFlagsResponse> resolve(
      EvaluationContext ctx, List<String> flagNames, boolean apply) {
    final long startNanos = System.nanoTime();
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

    return resolver
        .resolveProcess(
            ResolveProcessRequest.newBuilder()
                .setWithoutMaterializations(reqBuilder.build())
                .build())
        .thenApply(processResponse -> processResponse.getResolved().getResponse())
        .whenComplete(
            (response, error) -> {
              ResolveReason reason =
                  error != null
                      ? ResolveReason.RESOLVE_REASON_ERROR
                      : ResolveReason.RESOLVE_REASON_BUNDLE;
              doRegisterResolve(reason, startNanos);
            });
  }

  /**
   * Applies flags that were previously resolved with apply=false. This method is intended for use
   * by {@link FlagResolverService} to proxy apply requests from client SDKs. When
   * disableExposureCollection is configured, apply requests are ignored by the resolver.
   *
   * @param request the apply flags request containing resolve token and flags to apply
   */
  void applyFlags(ApplyFlagsRequest request) {
    resolver.applyFlags(request);
  }

  /**
   * Returns a Prometheus metrics snapshot aggregated from all resolver instances in the pool.
   *
   * <p><b>Experimental:</b> this API is subject to change.
   *
   * @param request the snapshot config (currently unused, reserved for future options)
   * @return the concatenated Prometheus metrics text from all pool slots
   */
  public String getPrometheusMetrics(SnapshotConfig request) {
    return resolver.prometheusSnapshot();
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
