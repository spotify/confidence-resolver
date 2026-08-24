package com.spotify.confidence.sdk;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;

public class LocalProviderConfig {
  /**
   * Default number of WASM resolver instances in the pool. The actual pool size is capped at {@code
   * Runtime.getRuntime().availableProcessors()}.
   */
  public static final int DEFAULT_RESOLVER_POOL_SIZE = 2;

  private final ChannelFactory channelFactory;
  private final HttpClientFactory httpClientFactory;
  private final boolean useRemoteMaterializationStore;
  private final int resolverPoolSize;
  private final String encryptionKey;
  private final boolean enableApplyDedup;
  private final boolean disableExposureCollection;
  private final byte[] eventWasmBytes;

  public LocalProviderConfig() {
    this(null, null);
  }

  public LocalProviderConfig(ChannelFactory channelFactory) {
    this(channelFactory, null);
  }

  public LocalProviderConfig(ChannelFactory channelFactory, HttpClientFactory httpClientFactory) {
    this(channelFactory, httpClientFactory, false, DEFAULT_RESOLVER_POOL_SIZE);
  }

  public LocalProviderConfig(
      ChannelFactory channelFactory,
      HttpClientFactory httpClientFactory,
      boolean useRemoteMaterializationStore) {
    this(channelFactory, httpClientFactory, useRemoteMaterializationStore, 0);
  }

  public LocalProviderConfig(
      ChannelFactory channelFactory,
      HttpClientFactory httpClientFactory,
      boolean useRemoteMaterializationStore,
      int resolverPoolSize) {
    this(channelFactory, httpClientFactory, useRemoteMaterializationStore, resolverPoolSize, null);
  }

  private LocalProviderConfig(
      ChannelFactory channelFactory,
      HttpClientFactory httpClientFactory,
      boolean useRemoteMaterializationStore,
      int resolverPoolSize,
      String encryptionKey) {
    this(
        channelFactory,
        httpClientFactory,
        useRemoteMaterializationStore,
        resolverPoolSize,
        encryptionKey,
        false,
        false,
        null);
  }

  private LocalProviderConfig(
      ChannelFactory channelFactory,
      HttpClientFactory httpClientFactory,
      boolean useRemoteMaterializationStore,
      int resolverPoolSize,
      String encryptionKey,
      boolean enableApplyDedup,
      boolean disableExposureCollection,
      byte[] eventWasmBytes) {
    this.channelFactory = channelFactory != null ? channelFactory : new DefaultChannelFactory();
    this.httpClientFactory =
        httpClientFactory != null ? httpClientFactory : new DefaultHttpClientFactory();
    this.useRemoteMaterializationStore = useRemoteMaterializationStore;
    this.resolverPoolSize = resolverPoolSize > 0 ? resolverPoolSize : DEFAULT_RESOLVER_POOL_SIZE;
    this.encryptionKey = encryptionKey;
    this.enableApplyDedup = enableApplyDedup;
    this.disableExposureCollection = disableExposureCollection;
    this.eventWasmBytes = eventWasmBytes;
  }

  public ChannelFactory getChannelFactory() {
    return channelFactory;
  }

  public HttpClientFactory getHttpClientFactory() {
    return httpClientFactory;
  }

  public boolean isUseRemoteMaterializationStore() {
    return useRemoteMaterializationStore;
  }

  /**
   * Returns the number of WASM resolver instances in the pool. Defaults to {@link
   * #DEFAULT_RESOLVER_POOL_SIZE}.
   */
  public int getResolverPoolSize() {
    return resolverPoolSize;
  }

  /** Returns the hex-encoded AES-256 encryption key, or {@code null} if unset. */
  public String getEncryptionKey() {
    return encryptionKey;
  }

  /** Experimental: returns whether apply-event deduplication in the WASM resolver is enabled. */
  public boolean isEnableApplyDedup() {
    return enableApplyDedup;
  }

  /**
   * Returns whether exposure/assignment collection is disabled for all OpenFeature evaluations
   * through this provider. This is intended only for exceptional no-exposure modes; resolve logs
   * and telemetry are still sent.
   */
  public boolean isDisableExposureCollection() {
    return disableExposureCollection;
  }

  /**
   * Returns the raw bytes of the event engine WASM binary, or {@code null} if event tracking is not
   * enabled.
   */
  public byte[] getEventWasmBytes() {
    return eventWasmBytes;
  }

  public static Builder builder() {
    return new Builder();
  }

  public static class Builder {
    private ChannelFactory channelFactory;
    private HttpClientFactory httpClientFactory;
    private boolean useRemoteMaterializationStore;
    private int resolverPoolSize;
    private String encryptionKey;
    private boolean enableApplyDedup;
    private boolean disableExposureCollection;
    private byte[] eventWasmBytes;

    public Builder channelFactory(ChannelFactory channelFactory) {
      this.channelFactory = channelFactory;
      return this;
    }

    public Builder httpClientFactory(HttpClientFactory httpClientFactory) {
      this.httpClientFactory = httpClientFactory;
      return this;
    }

    public Builder useRemoteMaterializationStore(boolean useRemoteMaterializationStore) {
      this.useRemoteMaterializationStore = useRemoteMaterializationStore;
      return this;
    }

    /**
     * Sets the number of WASM resolver instances in the pool. Increase for higher concurrency (with
     * the penalty of higher memory footprint). The value is capped at the number of available
     * processors. Defaults to {@link #DEFAULT_RESOLVER_POOL_SIZE}.
     *
     * @param resolverPoolSize the desired pool size
     */
    public Builder resolverPoolSize(int resolverPoolSize) {
      this.resolverPoolSize = resolverPoolSize;
      return this;
    }

    /** Sets the hex-encoded AES-256 encryption key for decrypting CDN state. */
    public Builder encryptionKey(String encryptionKey) {
      this.encryptionKey = encryptionKey;
      return this;
    }

    /**
     * Experimental: enables apply-event deduplication in the WASM resolver — repeated identical
     * assignments within a short TTL window are logged once. Off by default; the API may change.
     */
    public Builder enableApplyDedup(boolean enableApplyDedup) {
      this.enableApplyDedup = enableApplyDedup;
      return this;
    }

    /**
     * Disables exposure/assignment collection for all OpenFeature evaluations through this
     * provider. Use only for exceptional no-exposure modes; resolve logs and telemetry are still
     * sent.
     */
    public Builder disableExposureCollection(boolean disableExposureCollection) {
      this.disableExposureCollection = disableExposureCollection;
      return this;
    }

    /**
     * Sets the event engine WASM binary bytes. When set, the provider enables event tracking via
     * {@code track()} and periodically flushes events to the Confidence events API.
     *
     * @param eventWasmBytes the raw bytes of the {@code confidence_event_engine.wasm} binary
     */
    public Builder eventWasmBytes(byte[] eventWasmBytes) {
      this.eventWasmBytes = eventWasmBytes;
      return this;
    }

    /**
     * Loads the event engine WASM binary from the given file path. Convenience alternative to
     * {@link #eventWasmBytes(byte[])}.
     *
     * @param eventWasmPath path to the {@code confidence_event_engine.wasm} file
     * @throws IOException if the file cannot be read
     */
    public Builder eventWasmPath(Path eventWasmPath) throws IOException {
      this.eventWasmBytes = Files.readAllBytes(eventWasmPath);
      return this;
    }

    public LocalProviderConfig build() {
      return new LocalProviderConfig(
          channelFactory,
          httpClientFactory,
          useRemoteMaterializationStore,
          resolverPoolSize,
          encryptionKey,
          enableApplyDedup,
          disableExposureCollection,
          eventWasmBytes);
    }
  }
}
