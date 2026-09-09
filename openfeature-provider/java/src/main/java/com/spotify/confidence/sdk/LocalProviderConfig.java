package com.spotify.confidence.sdk;

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
  private final boolean enableApplyDedup;
  private final boolean disableExposureCollection;

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
    this(
        channelFactory,
        httpClientFactory,
        useRemoteMaterializationStore,
        DEFAULT_RESOLVER_POOL_SIZE);
  }

  public LocalProviderConfig(
      ChannelFactory channelFactory,
      HttpClientFactory httpClientFactory,
      boolean useRemoteMaterializationStore,
      int resolverPoolSize) {
    this(
        channelFactory,
        httpClientFactory,
        useRemoteMaterializationStore,
        resolverPoolSize,
        true,
        false);
  }

  /**
   * Overload that exposes apply-event deduplication. Dedup is on by default, so this is only needed
   * to turn it off — every other constructor leaves it enabled. {@link
   * Builder#enableApplyDedup(boolean)} does the same thing and is the preferred entry point for new
   * code.
   *
   * @param enableApplyDedup false to log every apply instead of collapsing repeated identical
   *     assignments for the same unit and variant within the dedup TTL window
   */
  public LocalProviderConfig(
      ChannelFactory channelFactory,
      HttpClientFactory httpClientFactory,
      boolean useRemoteMaterializationStore,
      int resolverPoolSize,
      boolean enableApplyDedup) {
    this(
        channelFactory,
        httpClientFactory,
        useRemoteMaterializationStore,
        resolverPoolSize,
        enableApplyDedup,
        false);
  }

  private LocalProviderConfig(
      ChannelFactory channelFactory,
      HttpClientFactory httpClientFactory,
      boolean useRemoteMaterializationStore,
      int resolverPoolSize,
      boolean enableApplyDedup,
      boolean disableExposureCollection) {
    this.channelFactory = channelFactory != null ? channelFactory : new DefaultChannelFactory();
    this.httpClientFactory =
        httpClientFactory != null ? httpClientFactory : new DefaultHttpClientFactory();
    this.useRemoteMaterializationStore = useRemoteMaterializationStore;
    this.resolverPoolSize = resolverPoolSize > 0 ? resolverPoolSize : DEFAULT_RESOLVER_POOL_SIZE;
    this.enableApplyDedup = enableApplyDedup;
    this.disableExposureCollection = disableExposureCollection;
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

  /** Returns whether apply-event deduplication in the WASM resolver is enabled (on by default). */
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

  public static Builder builder() {
    return new Builder();
  }

  public static class Builder {
    private ChannelFactory channelFactory;
    private HttpClientFactory httpClientFactory;
    private boolean useRemoteMaterializationStore;
    private int resolverPoolSize;
    private boolean enableApplyDedup = true;
    private boolean disableExposureCollection;

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

    /**
     * Apply-event deduplication in the WASM resolver — repeated identical assignments within a
     * short TTL window are logged once. On by default. Set to false to disable.
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

    public LocalProviderConfig build() {
      return new LocalProviderConfig(
          channelFactory,
          httpClientFactory,
          useRemoteMaterializationStore,
          resolverPoolSize,
          enableApplyDedup,
          disableExposureCollection);
    }
  }
}
