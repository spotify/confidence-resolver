package com.spotify.confidence.sdk;

import com.spotify.confidence.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.flags.resolver.v1.Sdk;
import com.spotify.confidence.flags.resolver.v1.SdkId;
import com.spotify.confidence.flags.types.v1.FlagSchema;
import com.spotify.confidence.flags.wasm.v1.EvaluateRequest;
import com.spotify.confidence.flags.wasm.v1.ResolutionDetails;
import com.spotify.confidence.sdk.OpenFeatureValueMapper.EncodedDefault;
import com.spotify.confidence.sdk.internal.engine.Engine;
import com.spotify.confidence.sdk.internal.engine.EngineClosedException;
import com.spotify.confidence.sdk.internal.engine.EngineConfig;
import com.spotify.confidence.sdk.internal.engine.NotReadyException;
import com.spotify.confidence.sdk.internal.engine.StartupTimeoutException;
import com.spotify.confidence.sdk.internal.engine.TerminalStartupException;
import dev.openfeature.sdk.ErrorCode;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.FeatureProvider;
import dev.openfeature.sdk.Metadata;
import dev.openfeature.sdk.ProviderEvaluation;
import dev.openfeature.sdk.ProviderState;
import dev.openfeature.sdk.TrackingEventDetails;
import dev.openfeature.sdk.Value;
import dev.openfeature.sdk.exceptions.GeneralError;
import dev.openfeature.sdk.exceptions.TypeMismatchError;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import org.slf4j.Logger;

/**
 * OpenFeature provider resolving flags locally in-process: flag state is compiled to a WebAssembly
 * module fetched from the Confidence CDN, and evaluation happens on the JVM through the state-to-
 * wasm engine — no resolver service round trip per evaluation.
 *
 * <p>Exposures (flag assignments) are logged to Confidence ingestion, deduplicated per default
 * process (see {@link com.spotify.confidence.sdk.internal.engine.ExposureMode}); sticky assignments
 * and materialized segments persist through the configured {@link MaterializationStore}.
 *
 * <p>Lifecycle: after construction the provider is {@link ProviderState#NOT_READY} and evaluations
 * return caller defaults; {@link #initialize} installs the first module (throwing on a terminal
 * credential rejection, or on an initialization timeout while the engine keeps retrying in the
 * background); {@link #shutdown} flushes buffered logs and releases the engine within 10 seconds.
 *
 * <p>Unsupported, as in earlier versions: the resolve token is neither encrypted nor authenticated,
 * so treat it as untrusted input when applying flags.
 */
public class OpenFeatureLocalResolveProvider implements FeatureProvider {

  /** The provider name reported through {@link #getMetadata()}. */
  public static final String PROVIDER_NAME = "confidence-sdk-java-local";

  /** The budget for flushing logs and releasing the engine on {@link #shutdown()}. */
  private static final Duration SHUTDOWN_GRACE = Duration.ofSeconds(10);

  private static final Sdk SDK =
      Sdk.newBuilder().setId(SdkId.SDK_ID_JAVA_LOCAL_PROVIDER).setVersion(Version.VERSION).build();

  private final String clientSecret;
  private final Engine engine;
  private final com.spotify.confidence.sdk.internal.host.MaterializationStore hostStore;
  private final RemoteMaterializationStore createdRemoteStore;
  private final ExecutorService writeExecutor;
  private final Duration initializationTimeout;
  private final Logger logger;
  private final AtomicReference<ProviderState> state =
      new AtomicReference<>(ProviderState.NOT_READY);
  private final AtomicBoolean closed = new AtomicBoolean();

  /**
   * Creates a provider with the default configuration: deduplicated exposures, one resolver
   * instance per processor, and no materialization store (sticky assignments fall back to
   * non-sticky evaluation).
   */
  public OpenFeatureLocalResolveProvider(String clientSecret, String encryptionKey) {
    this(LocalProviderConfig.builder().build(), clientSecret, encryptionKey, null);
  }

  public OpenFeatureLocalResolveProvider(
      LocalProviderConfig config, String clientSecret, String encryptionKey) {
    this(config, clientSecret, encryptionKey, null);
  }

  /**
   * Creates a provider with a materialization store for sticky assignments and materialized
   * segments.
   */
  public OpenFeatureLocalResolveProvider(
      String clientSecret, String encryptionKey, MaterializationStore materializationStore) {
    this(LocalProviderConfig.builder().build(), clientSecret, encryptionKey, materializationStore);
  }

  public OpenFeatureLocalResolveProvider(
      LocalProviderConfig config,
      String clientSecret,
      String encryptionKey,
      MaterializationStore materializationStore) {
    this(
        config == null ? LocalProviderConfig.builder().build() : config,
        clientSecret,
        encryptionKey,
        materializationStore,
        true);
  }

  private OpenFeatureLocalResolveProvider(
      LocalProviderConfig config,
      String clientSecret,
      String encryptionKey,
      MaterializationStore explicitStore,
      boolean unused) {
    if (clientSecret == null || clientSecret.isBlank()) {
      throw new IllegalArgumentException("clientSecret must not be empty");
    }
    if (encryptionKey == null || !encryptionKey.matches("[0-9a-fA-F]{64}")) {
      throw new IllegalArgumentException(
          "encryptionKey must be 64 hexadecimal characters (32 bytes)");
    }
    if (explicitStore != null && config.getMaterializationStore() != null) {
      throw new IllegalArgumentException(
          "materializationStore was set both on the config and as a constructor argument");
    }
    if ((explicitStore != null || config.getMaterializationStore() != null)
        && config.isUseRemoteMaterializationStore()) {
      throw new IllegalArgumentException(
          "materializationStore and useRemoteMaterializationStore are mutually exclusive");
    }
    this.clientSecret = clientSecret;
    this.initializationTimeout = config.getInitializationTimeout();
    this.logger = config.getLogger();
    java.net.http.HttpClient httpClient = config.getHttpClientFactory().createHttpClient();
    this.writeExecutor =
        Executors.newCachedThreadPool(
            r -> {
              Thread t = new Thread(r, "confidence-materialization-write");
              t.setDaemon(true);
              return t;
            });
    RemoteMaterializationStore remote = null;
    MaterializationStore store =
        explicitStore != null ? explicitStore : config.getMaterializationStore();
    if (store == null && config.isUseRemoteMaterializationStore()) {
      remote =
          new RemoteMaterializationStore(
              clientSecret, config.getRemoteMaterializationStoreUrl(), httpClient);
      store = remote;
    }
    this.createdRemoteStore = remote;
    this.hostStore =
        new StoreAdapter(
            store == null ? new UnsupportedMaterializationStore() : store, logger, writeExecutor);

    EngineConfig.Builder engine =
        new EngineConfig.Builder()
            .clientSecret(clientSecret)
            .encryptionKey(hexToBytes(encryptionKey))
            .httpClient(httpClient)
            .resolverBaseUrl(config.getResolverBaseUrl())
            .exposure(config.getExposureMode())
            .interpreter(config.isInterpreter())
            .logger(logger)
            .onReady(
                () -> {
                  state.set(ProviderState.READY);
                  logger.info("confidence provider ready");
                });
    if (config.getInstances() > 0) {
      engine.instances(config.getInstances());
    } else {
      // One instance per processor, as the Go provider's GOMAXPROCS default.
      engine.instances(Runtime.getRuntime().availableProcessors());
    }
    if (config.getMaxMemoryBytes() > 0) {
      engine.maxMemoryBytes(config.getMaxMemoryBytes());
    }
    Duration poll = config.getStatePollInterval();
    if (poll != null && !poll.isZero() && !poll.isNegative()) {
      engine.pollInterval(poll);
    }
    Duration retry = config.getRetryInterval();
    if (retry != null && !retry.isZero() && !retry.isNegative()) {
      engine.retryInterval(retry);
    }
    if (!config.getCdnBaseUrl().isEmpty()) {
      engine.cdnBaseUrl(config.getCdnBaseUrl());
    }
    if (!config.getIngestBaseUrl().isEmpty()) {
      engine.ingestBaseUrl(config.getIngestBaseUrl());
    }
    this.engine = new Engine(engine.build());
  }

  private static byte[] hexToBytes(String hex) {
    byte[] out = new byte[hex.length() / 2];
    for (int i = 0; i < out.length; i++) {
      out[i] =
          (byte)
              ((Character.digit(hex.charAt(2 * i), 16) << 4)
                  | Character.digit(hex.charAt(2 * i + 1), 16));
    }
    return out;
  }

  // ------------------------------------------------------------------ lifecycle

  @Override
  public ProviderState getState() {
    return state.get();
  }

  @Override
  public Metadata getMetadata() {
    return () -> PROVIDER_NAME;
  }

  /**
   * Installs the first resolver module, waiting up to the configuration's initialization timeout. A
   * {@link TerminalStartupException} (the CDN rejected the credential) surfaces as a {@link
   * GeneralError} and the provider will not become ready; a timeout throws while the engine keeps
   * retrying in the background — the provider becomes {@link ProviderState#READY} on recovery.
   */
  @Override
  public void initialize(EvaluationContext evaluationContext) {
    if (closed.get()) {
      return;
    }
    try {
      engine.start(initializationTimeout);
      state.set(ProviderState.READY);
    } catch (TerminalStartupException e) {
      state.set(ProviderState.FATAL);
      throw new GeneralError("module CDN rejected the credential: " + e.getMessage());
    } catch (StartupTimeoutException e) {
      // The engine keeps polling; onReady flips the state when the first module lands.
      logger.warn("confidence provider initialization timed out; retrying in the background");
      throw new GeneralError(e.getMessage());
    }
  }

  /** Flushes buffered flag logs and releases the engine. Idempotent. */
  @Override
  public void shutdown() {
    if (!closed.compareAndSet(false, true)) {
      return;
    }
    state.set(ProviderState.NOT_READY);
    try {
      if (!engine.close(SHUTDOWN_GRACE)) {
        logger.warn("provider shutdown hit the {} budget", SHUTDOWN_GRACE);
      }
    } catch (RuntimeException e) {
      logger.warn("provider shutdown", e);
    }
    writeExecutor.shutdown();
    try {
      if (!writeExecutor.awaitTermination(5, TimeUnit.SECONDS)) {
        writeExecutor.shutdownNow();
      }
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      writeExecutor.shutdownNow();
    }
    if (createdRemoteStore != null) {
      createdRemoteStore.shutdown();
    }
    FeatureProvider.super.shutdown();
  }

  @Override
  public void track(
      String trackingEventName, EvaluationContext context, TrackingEventDetails details) {
    logger.warn("event tracking is not supported by the local resolver provider yet");
  }

  // ---------------------------------------------------------------- evaluation

  @Override
  public ProviderEvaluation<Boolean> getBooleanEvaluation(
      String key, Boolean defaultValue, EvaluationContext ctx) {
    return evaluate(key, defaultValue, ctx, false);
  }

  @Override
  public ProviderEvaluation<String> getStringEvaluation(
      String key, String defaultValue, EvaluationContext ctx) {
    return evaluate(key, defaultValue, ctx, null);
  }

  @Override
  public ProviderEvaluation<Integer> getIntegerEvaluation(
      String key, Integer defaultValue, EvaluationContext ctx) {
    return evaluate(key, defaultValue, ctx, 0);
  }

  @Override
  public ProviderEvaluation<Double> getDoubleEvaluation(
      String key, Double defaultValue, EvaluationContext ctx) {
    return evaluate(key, defaultValue, ctx, 0.0);
  }

  @Override
  public ProviderEvaluation<Value> getObjectEvaluation(
      String key, Value defaultValue, EvaluationContext ctx) {
    return evaluate(key, defaultValue, ctx, new Value());
  }

  /**
   * One evaluation path for every type, as the Go provider: encode the default with its schema,
   * resolve the flag (name before the first dot; the rest is a value path the module evaluates),
   * check every error code, and decode against the default's shape.
   *
   * @param zero substitutes for a null default when the module resolved to null
   */
  private <T> ProviderEvaluation<T> evaluate(
      String flag, T defaultValue, EvaluationContext ctx, T zero) {
    T def = defaultValue == null ? zero : defaultValue;
    Value defValue = wrap(def);
    try {
      EncodedDefault encoded = OpenFeatureValueMapper.encodeDefault(defValue);
      ResolutionDetails details;
      try {
        details = evaluateOne(flag, defValue, encoded, ctx);
      } catch (NotReadyException e) {
        return failure(def, ErrorCode.PROVIDER_NOT_READY, "provider is not ready");
      } catch (EngineClosedException e) {
        return failure(def, ErrorCode.PROVIDER_FATAL, "provider is closed");
      } catch (InvalidContextException e) {
        return failure(def, ErrorCode.INVALID_CONTEXT, e.getMessage());
      } catch (MaterializationRequiredException e) {
        logger.warn(
            "flag {} needs materializations, but no MaterializationStore is configured", flag);
        return failure(def, ErrorCode.GENERAL, e.getMessage());
      } catch (RuntimeException e) {
        logger.error("flag resolve failed: {}", flag, e);
        return failure(def, ErrorCode.GENERAL, e.getMessage());
      }
      if (details.getErrorCode()
          != com.spotify.confidence.flags.wasm.v1.ErrorCode.ERROR_CODE_UNSPECIFIED) {
        return failure(def, resolutionError(details.getErrorCode()), details.getErrorMessage());
      }
      Value resolved = OpenFeatureValueMapper.decode(details.getValue(), defValue);
      return success(extract(resolved, def), details);
    } catch (TypeMismatchError e) {
      return failure(def, ErrorCode.TYPE_MISMATCH, e.getMessage());
    }
  }

  private ResolutionDetails evaluateOne(
      String flag, Value defValue, EncodedDefault encoded, EvaluationContext ctx) {
    String flagName = cut(flag);
    ResolveFlagsRequest.Builder request =
        ResolveFlagsRequest.newBuilder()
            .addFlags("flags/" + flagName)
            .setClientSecret(clientSecret)
            .setApply(!OpenFeatureValueMapper.skipApply(ctx))
            .setSdk(SDK);
    try {
      request.setEvaluationContext(OpenFeatureValueMapper.toProtoContext(ctx));
    } catch (RuntimeException e) {
      throw new InvalidContextException(e.getMessage(), e);
    }
    EvaluateRequest eval =
        EvaluateRequest.newBuilder()
            .setFlagKey(flag)
            .setDefaultValue(encoded.value())
            .setExpectedSchema(
                encoded.schema() == null ? FlagSchema.getDefaultInstance() : encoded.schema())
            .build();
    try {
      List<ResolutionDetails> details = engine.evaluate(request.build(), List.of(eval), hostStore);
      return details.get(0);
    } catch (RuntimeException e) {
      throw unwrapStoreError(e);
    }
  }

  private static RuntimeException unwrapStoreError(RuntimeException e) {
    Throwable cause = e.getCause();
    while (cause != null) {
      if (cause instanceof MaterializationNotSupportedException) {
        return new MaterializationRequiredException();
      }
      cause = cause.getCause();
    }
    return e;
  }

  private <T> ProviderEvaluation<T> success(T value, ResolutionDetails details) {
    ProviderEvaluation<T> out = new ProviderEvaluation<>();
    out.setValue(value);
    out.setVariant(details.getVariant());
    out.setReason(reason(details.getReason()).name());
    return out;
  }

  /**
   * Maps the module's resolve reason onto the OpenFeature reasons, as the Go provider's reason().
   */
  private static dev.openfeature.sdk.Reason reason(
      com.spotify.confidence.flags.resolver.v1.ResolveReason r) {
    return switch (r) {
      case RESOLVE_REASON_MATCH -> dev.openfeature.sdk.Reason.TARGETING_MATCH;
      case RESOLVE_REASON_NO_SEGMENT_MATCH -> dev.openfeature.sdk.Reason.DEFAULT;
      case RESOLVE_REASON_FLAG_ARCHIVED -> dev.openfeature.sdk.Reason.DISABLED;
      case RESOLVE_REASON_TARGETING_KEY_ERROR,
              RESOLVE_REASON_ERROR,
              RESOLVE_REASON_UNRECOGNIZED_TARGETING_RULE ->
          dev.openfeature.sdk.Reason.ERROR;
      default -> dev.openfeature.sdk.Reason.UNKNOWN;
    };
  }

  private static <T> ProviderEvaluation<T> failure(T def, ErrorCode code, String message) {
    ProviderEvaluation<T> out = new ProviderEvaluation<>();
    out.setValue(def);
    out.setErrorCode(code);
    out.setErrorMessage(message);
    out.setReason(dev.openfeature.sdk.Reason.ERROR.name());
    return out;
  }

  /** {@code a.b.c} resolves flag {@code a} at value path {@code b.c}, as in earlier versions. */
  private static String cut(String flag) {
    int i = flag.indexOf('.');
    return i < 0 ? flag : flag.substring(0, i);
  }

  private static dev.openfeature.sdk.ErrorCode resolutionError(
      com.spotify.confidence.flags.wasm.v1.ErrorCode code) {
    return switch (code) {
      case ERROR_CODE_FLAG_NOT_FOUND -> ErrorCode.FLAG_NOT_FOUND;
      case ERROR_CODE_TYPE_MISMATCH -> ErrorCode.TYPE_MISMATCH;
      case ERROR_CODE_PROVIDER_NOT_READY -> ErrorCode.PROVIDER_NOT_READY;
      case ERROR_CODE_PROVIDER_FATAL -> ErrorCode.PROVIDER_FATAL;
      default -> ErrorCode.GENERAL;
    };
  }

  private static dev.openfeature.sdk.Value wrap(Object value) {
    if (value instanceof Value v) {
      return v;
    }
    if (value == null) {
      return new Value();
    }
    if (value instanceof Boolean b) {
      return new Value(b);
    }
    if (value instanceof String s) {
      return new Value(s);
    }
    if (value instanceof Integer i) {
      return new Value(i);
    }
    if (value instanceof Double d) {
      return new Value(d);
    }
    return Value.objectToValue(value);
  }

  @SuppressWarnings("unchecked")
  private static <T> T extract(dev.openfeature.sdk.Value resolved, T def) {
    if (def instanceof Value) {
      return (T) resolved;
    }
    if (resolved.isNull()) {
      return def;
    }
    if (def == null) {
      // A null default: yield the natural representation.
      return (T) resolved.asObject();
    }
    if (def instanceof Boolean) {
      if (!resolved.isBoolean()) {
        throw new TypeMismatchError("resolved value is not a boolean");
      }
      return (T) resolved.asBoolean();
    }
    if (def instanceof String) {
      if (!resolved.isString()) {
        throw new TypeMismatchError("resolved value is not a string");
      }
      return (T) resolved.asString();
    }
    if (def instanceof Integer) {
      if (!(resolved.isNumber() && resolved.asObject() instanceof Integer)) {
        throw new TypeMismatchError("resolved value is not an integer");
      }
      return (T) resolved.asInteger();
    }
    if (def instanceof Double) {
      if (!resolved.isNumber()) {
        throw new TypeMismatchError("resolved value is not a number");
      }
      return (T) resolved.asDouble();
    }
    return (T) resolved.asObject();
  }

  // --------------------------------------------------------- resolve service API

  /**
   * Resolves flags by name (the {@code flags/} prefix is added when missing) and returns the full
   * response, including the resolve token for deferred apply. When {@code apply} is false, the
   * returned token is neither encrypted nor authenticated — treat it as untrusted input.
   */
  public CompletionStage<ResolveFlagsResponse> resolve(
      EvaluationContext ctx, List<String> flagNames, boolean apply) {
    try {
      ResolveFlagsRequest.Builder request =
          ResolveFlagsRequest.newBuilder()
              .setClientSecret(clientSecret)
              .setApply(apply)
              .setSdk(SDK);
      for (String name : flagNames) {
        request.addFlags(name.startsWith("flags/") ? name : "flags/" + name);
      }
      request.setEvaluationContext(OpenFeatureValueMapper.toProtoContext(ctx));
      return CompletableFuture.completedFuture(engine.resolve(request.build(), hostStore));
    } catch (RuntimeException e) {
      CompletableFuture<ResolveFlagsResponse> failed = new CompletableFuture<>();
      failed.completeExceptionally(e);
      return failed;
    }
  }

  /**
   * Applies flags resolved with {@code apply=false}: sends the flag assignments to Confidence
   * ingestion with the resolve token. Failures are swallowed and logged, as in earlier versions —
   * applies are best-effort telemetry.
   */
  public void applyFlags(ApplyFlagsRequest request) {
    if (request == null) {
      return;
    }
    ApplyFlagsRequest filled =
        request.toBuilder()
            .setClientSecret(
                request.getClientSecret().isEmpty() ? clientSecret : request.getClientSecret())
            .build();
    if (!filled.getSdk().hasId() && !filled.getSdk().hasCustomId()) {
      filled = filled.toBuilder().setSdk(SDK).build();
    }
    try {
      engine.applyFlags(filled.toByteArray());
    } catch (NotReadyException | EngineClosedException e) {
      logger.warn("Failed to apply flags: {}", e.getMessage());
    } catch (RuntimeException e) {
      logger.warn("Failed to apply flags", e);
    }
  }

  /**
   * The module's cumulative telemetry in Prometheus text format, or OpenMetrics format when the
   * config asks for it. Empty before the first module.
   *
   * <p><b>Experimental:</b> this API is subject to change.
   */
  public String getPrometheusMetrics(SnapshotConfig config) {
    return engine.metrics(config != null && config.isOpenMetrics());
  }

  /** Not-ready from the engine: evaluations serve defaults. */
  private static final class InvalidContextException extends RuntimeException {
    InvalidContextException(String message, Throwable cause) {
      super(message, cause);
    }
  }

  private static final class MaterializationRequiredException extends RuntimeException {
    MaterializationRequiredException() {
      super("flag requires materializations; configure a MaterializationStore");
    }
  }
}
