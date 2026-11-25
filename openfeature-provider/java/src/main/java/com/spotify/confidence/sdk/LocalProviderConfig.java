package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.internal.engine.ExposureMode;
import java.time.Duration;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Configuration for {@link OpenFeatureLocalResolveProvider}.
 *
 * <p>Defaults mirror the state-to-wasm engine: one resolver instance per available processor, a 64
 * MiB memory cap per module generation, a 10-second module poll, a 30-second initialization
 * timeout, and deduplicated exposure logging.
 *
 * <p>Example:
 *
 * <pre>{@code
 * LocalProviderConfig config = LocalProviderConfig.builder()
 *     .materializationStore(new MyStore())
 *     .exposureMode(ExposureMode.ALL)
 *     .statePollInterval(Duration.ofSeconds(30))
 *     .build();
 * OpenFeatureLocalResolveProvider provider =
 *     new OpenFeatureLocalResolveProvider(config, clientSecret, encryptionKey);
 * }</pre>
 */
public final class LocalProviderConfig {

  /** The default initialization budget, as in the engine's start. */
  public static final Duration DEFAULT_INITIALIZATION_TIMEOUT = Duration.ofSeconds(30);

  private final MaterializationStore materializationStore;
  private final boolean useRemoteMaterializationStore;
  private final String remoteMaterializationStoreUrl;
  private final int instances;
  private final long maxMemoryBytes;
  private final Duration statePollInterval;
  private final Duration initializationTimeout;
  private final ExposureMode exposureMode;
  private final boolean interpreter;
  private final Duration retryInterval;
  private final String cdnBaseUrl;
  private final String resolverBaseUrl;
  private final String ingestBaseUrl;
  private final HttpClientFactory httpClientFactory;
  private final Logger logger;

  private LocalProviderConfig(Builder b) {
    this.materializationStore = b.materializationStore;
    this.useRemoteMaterializationStore = b.useRemoteMaterializationStore;
    this.remoteMaterializationStoreUrl = b.remoteMaterializationStoreUrl;
    this.instances = b.instances;
    this.maxMemoryBytes = b.maxMemoryBytes;
    this.statePollInterval = b.statePollInterval;
    this.initializationTimeout = b.initializationTimeout;
    this.exposureMode = b.exposureMode;
    this.interpreter = b.interpreter;
    this.retryInterval = b.retryInterval;
    this.cdnBaseUrl = b.cdnBaseUrl;
    this.resolverBaseUrl = b.resolverBaseUrl;
    this.ingestBaseUrl = b.ingestBaseUrl;
    this.httpClientFactory = b.httpClientFactory;
    this.logger = b.logger;
  }

  public static Builder builder() {
    return new Builder();
  }

  /** The configured store, or {@code null} when none was set (the provider picks the default). */
  public MaterializationStore getMaterializationStore() {
    return materializationStore;
  }

  public boolean isUseRemoteMaterializationStore() {
    return useRemoteMaterializationStore;
  }

  /** Overrides the hosted materialization store's base URL; empty means the production default. */
  public String getRemoteMaterializationStoreUrl() {
    return remoteMaterializationStoreUrl;
  }

  /** Resolver instances; zero or negative keeps the engine default (one per processor). */
  public int getInstances() {
    return instances;
  }

  /** Module memory cap in bytes, per installed generation; zero or negative keeps the default. */
  public long getMaxMemoryBytes() {
    return maxMemoryBytes;
  }

  public Duration getStatePollInterval() {
    return statePollInterval;
  }

  public Duration getInitializationTimeout() {
    return initializationTimeout;
  }

  public ExposureMode getExposureMode() {
    return exposureMode;
  }

  /** Runs the module on the interpreter instead of the compiled runtime. */
  public boolean isInterpreter() {
    return interpreter;
  }

  /** The CDN retry interval; zero keeps the engine default. */
  public Duration getRetryInterval() {
    return retryInterval;
  }

  /** Overrides the module CDN base URL (for tests); empty means the production default. */
  public String getCdnBaseUrl() {
    return cdnBaseUrl;
  }

  /** Overrides the resolver base URL for module profile registration. */
  public String getResolverBaseUrl() {
    return resolverBaseUrl;
  }

  /** Overrides the flag-log ingestion base URL (for tests); empty means the production default. */
  public String getIngestBaseUrl() {
    return ingestBaseUrl;
  }

  public HttpClientFactory getHttpClientFactory() {
    return httpClientFactory;
  }

  public Logger getLogger() {
    return logger;
  }

  /** Builder for {@link LocalProviderConfig}. */
  public static final class Builder {

    private MaterializationStore materializationStore;
    private boolean useRemoteMaterializationStore;
    private String remoteMaterializationStoreUrl = "";
    private int instances;
    private long maxMemoryBytes;
    private Duration statePollInterval;
    private Duration initializationTimeout = DEFAULT_INITIALIZATION_TIMEOUT;
    private ExposureMode exposureMode = ExposureMode.DEDUPLICATED;
    private boolean interpreter;
    private Duration retryInterval;
    private String cdnBaseUrl = "";
    private String resolverBaseUrl = "";
    private String ingestBaseUrl = "";
    private HttpClientFactory httpClientFactory = HttpClientFactory.defaultFactory();
    private Logger logger = LoggerFactory.getLogger(OpenFeatureLocalResolveProvider.class);

    /**
     * A custom store for sticky assignments and materialized segments. Mutually exclusive with
     * {@link #useRemoteMaterializationStore(boolean)}.
     */
    public Builder materializationStore(MaterializationStore materializationStore) {
      this.materializationStore = materializationStore;
      return this;
    }

    /**
     * Uses the hosted {@link RemoteMaterializationStore} instead of a local one. Mutually exclusive
     * with {@link #materializationStore(MaterializationStore)}.
     */
    public Builder useRemoteMaterializationStore(boolean useRemoteMaterializationStore) {
      this.useRemoteMaterializationStore = useRemoteMaterializationStore;
      return this;
    }

    /** Overrides the hosted store's base URL (for tests). */
    public Builder remoteMaterializationStoreUrl(String url) {
      this.remoteMaterializationStoreUrl = url == null ? "" : url;
      return this;
    }

    /** Resolver instances; defaults to one per available processor. */
    public Builder instances(int instances) {
      this.instances = instances;
      return this;
    }

    /** Module memory cap in bytes, per installed generation; defaults to 64 MiB. */
    public Builder maxMemoryBytes(long maxMemoryBytes) {
      this.maxMemoryBytes = maxMemoryBytes;
      return this;
    }

    /** How often to check the CDN for a new module; defaults to 10 seconds. */
    public Builder statePollInterval(Duration statePollInterval) {
      this.statePollInterval = statePollInterval;
      return this;
    }

    /** How long {@code initialize()} may wait for the first module; defaults to 30 seconds. */
    public Builder initializationTimeout(Duration initializationTimeout) {
      this.initializationTimeout = initializationTimeout;
      return this;
    }

    /** How exposures are logged: {@code DEDUPLICATED} (default), {@code ALL}, or {@code NONE}. */
    public Builder exposureMode(ExposureMode exposureMode) {
      this.exposureMode = exposureMode == null ? ExposureMode.DEDUPLICATED : exposureMode;
      return this;
    }

    /** Runs the module on the interpreter; the default is the compiled runtime. */
    public Builder interpreter(boolean interpreter) {
      this.interpreter = interpreter;
      return this;
    }

    /** How quickly a failed module fetch retries; defaults to 1 second. */
    public Builder retryInterval(Duration retryInterval) {
      this.retryInterval = retryInterval;
      return this;
    }

    /** Overrides the module CDN base URL (for tests). */
    public Builder cdnBaseUrl(String cdnBaseUrl) {
      this.cdnBaseUrl = cdnBaseUrl == null ? "" : cdnBaseUrl;
      return this;
    }

    /**
     * Overrides the resolver base URL for module profile registration; defaults to
     * resolver.confidence.dev.
     */
    public Builder resolverBaseUrl(String resolverBaseUrl) {
      this.resolverBaseUrl = resolverBaseUrl == null ? "" : resolverBaseUrl;
      return this;
    }

    /** Overrides the flag-log ingestion base URL (for tests). */
    public Builder ingestBaseUrl(String ingestBaseUrl) {
      this.ingestBaseUrl = ingestBaseUrl == null ? "" : ingestBaseUrl;
      return this;
    }

    public Builder httpClientFactory(HttpClientFactory httpClientFactory) {
      this.httpClientFactory =
          httpClientFactory == null ? HttpClientFactory.defaultFactory() : httpClientFactory;
      return this;
    }

    public Builder logger(Logger logger) {
      this.logger =
          logger == null ? LoggerFactory.getLogger(OpenFeatureLocalResolveProvider.class) : logger;
      return this;
    }

    public LocalProviderConfig build() {
      if (materializationStore != null && useRemoteMaterializationStore) {
        throw new IllegalArgumentException(
            "materializationStore and useRemoteMaterializationStore are mutually exclusive");
      }
      if (initializationTimeout == null
          || initializationTimeout.isZero()
          || initializationTimeout.isNegative()) {
        initializationTimeout = DEFAULT_INITIALIZATION_TIMEOUT;
      }
      return new LocalProviderConfig(this);
    }
  }
}
