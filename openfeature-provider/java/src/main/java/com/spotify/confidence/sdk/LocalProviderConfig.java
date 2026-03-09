package com.spotify.confidence.sdk;

public class LocalProviderConfig {
  public static final int DEFAULT_RESOLVER_POOL_SIZE = 2;

  private final ChannelFactory channelFactory;
  private final HttpClientFactory httpClientFactory;
  private final boolean useRemoteMaterializationStore;
  private final int resolverPoolSize;

  public LocalProviderConfig() {
    this(null, null);
  }

  public LocalProviderConfig(ChannelFactory channelFactory) {
    this(channelFactory, null);
  }

  public LocalProviderConfig(ChannelFactory channelFactory, HttpClientFactory httpClientFactory) {
    this(channelFactory, httpClientFactory, false, 0);
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
    this.channelFactory = channelFactory != null ? channelFactory : new DefaultChannelFactory();
    this.httpClientFactory =
        httpClientFactory != null ? httpClientFactory : new DefaultHttpClientFactory();
    this.useRemoteMaterializationStore = useRemoteMaterializationStore;
    this.resolverPoolSize = resolverPoolSize > 0 ? resolverPoolSize : DEFAULT_RESOLVER_POOL_SIZE;
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

  public int getResolverPoolSize() {
    return resolverPoolSize;
  }

  public static Builder builder() {
    return new Builder();
  }

  public static class Builder {
    private ChannelFactory channelFactory;
    private HttpClientFactory httpClientFactory;
    private boolean useRemoteMaterializationStore;
    private int resolverPoolSize;

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

    public Builder resolverPoolSize(int resolverPoolSize) {
      this.resolverPoolSize = resolverPoolSize;
      return this;
    }

    public LocalProviderConfig build() {
      return new LocalProviderConfig(
          channelFactory, httpClientFactory, useRemoteMaterializationStore, resolverPoolSize);
    }
  }
}
