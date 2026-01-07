package com.spotify.confidence.sdk;

public class LocalProviderConfig {
  private final ChannelFactory channelFactory;
  private final HttpClientFactory httpClientFactory;
  private final boolean useRemoteMaterializationStore;

  public LocalProviderConfig() {
    this(null, null);
  }

  public LocalProviderConfig(ChannelFactory channelFactory) {
    this(channelFactory, null);
  }

  public LocalProviderConfig(ChannelFactory channelFactory, HttpClientFactory httpClientFactory) {
    this.channelFactory = channelFactory != null ? channelFactory : new DefaultChannelFactory();
    this.httpClientFactory =
        httpClientFactory != null ? httpClientFactory : new DefaultHttpClientFactory();
    this.useRemoteMaterializationStore = false;
  }

  public LocalProviderConfig(
      ChannelFactory channelFactory,
      HttpClientFactory httpClientFactory,
      boolean useRemoteMaterializationStore) {
    this.channelFactory = channelFactory != null ? channelFactory : new DefaultChannelFactory();
    this.httpClientFactory =
        httpClientFactory != null ? httpClientFactory : new DefaultHttpClientFactory();
    this.useRemoteMaterializationStore = useRemoteMaterializationStore;
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

  public static Builder builder() {
    return new Builder();
  }

  public static class Builder {
    private ChannelFactory channelFactory;
    private HttpClientFactory httpClientFactory;
    private boolean useRemoteMaterializationStore;

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

    public LocalProviderConfig build() {
      return new LocalProviderConfig(
          channelFactory, httpClientFactory, useRemoteMaterializationStore);
    }
  }
}
