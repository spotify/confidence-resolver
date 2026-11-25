package com.spotify.confidence.sdk.internal.engine;

import com.spotify.confidence.sdk.internal.host.Filter;
import java.net.http.HttpClient;
import java.time.Duration;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/** Configuration for an {@link Engine}. Zero values select defaults. */
public final class EngineConfig {

  public final String clientSecret;

  /** 32 bytes; see {@code CdnFetcher.parseKey}. */
  public final byte[] encryptionKey;

  public final HttpClient httpClient;
  public final String cdnBaseUrl;
  public final String resolverBaseUrl;
  public final String ingestBaseUrl;

  public final int instances;
  public final long maxMemoryBytes;
  public final boolean interpreter;
  public final ExposureMode exposure;

  /** Default 10s. */
  public final Duration pollInterval;

  /** Default 30s. */
  public final Duration flushInterval;

  /** Startup retry; default 1s. */
  public final Duration retryInterval;

  public final Logger logger;

  /** Called once, when the first module installs after {@code start} returned without one. */
  public final Runnable onReady;

  /** Overrides the apply filter the exposure mode selects (tests). */
  final Filter applyFilter;

  private EngineConfig(Builder b) {
    this.clientSecret = b.clientSecret;
    this.encryptionKey = b.encryptionKey;
    this.httpClient = b.httpClient;
    this.cdnBaseUrl = b.cdnBaseUrl;
    this.resolverBaseUrl = b.resolverBaseUrl;
    this.ingestBaseUrl = b.ingestBaseUrl;
    this.instances = b.instances;
    this.maxMemoryBytes = b.maxMemoryBytes;
    this.interpreter = b.interpreter;
    this.exposure = b.exposure;
    this.pollInterval = b.pollInterval;
    this.flushInterval = b.flushInterval;
    this.retryInterval = b.retryInterval;
    this.logger = b.logger;
    this.onReady = b.onReady;
    this.applyFilter = b.applyFilter;
  }

  public static final class Builder {

    private String clientSecret;
    private byte[] encryptionKey;
    private HttpClient httpClient;
    private String cdnBaseUrl = "";
    private String resolverBaseUrl = "";
    private String ingestBaseUrl = "";
    private int instances = 1;
    private long maxMemoryBytes = 64L << 20;
    private boolean interpreter = false;
    private ExposureMode exposure = ExposureMode.DEDUPLICATED;
    private Duration pollInterval = Duration.ofSeconds(10);
    private Duration flushInterval = Duration.ofSeconds(30);
    private Duration retryInterval = Duration.ofSeconds(1);
    private Logger logger = LoggerFactory.getLogger(EngineConfig.class);
    private Runnable onReady = () -> {};
    private Filter applyFilter;

    public Builder clientSecret(String clientSecret) {
      this.clientSecret = clientSecret;
      return this;
    }

    public Builder encryptionKey(byte[] encryptionKey) {
      this.encryptionKey = encryptionKey;
      return this;
    }

    public Builder httpClient(HttpClient httpClient) {
      this.httpClient = httpClient;
      return this;
    }

    public Builder cdnBaseUrl(String cdnBaseUrl) {
      this.cdnBaseUrl = cdnBaseUrl;
      return this;
    }

    public Builder resolverBaseUrl(String resolverBaseUrl) {
      this.resolverBaseUrl = resolverBaseUrl;
      return this;
    }

    public Builder ingestBaseUrl(String ingestBaseUrl) {
      this.ingestBaseUrl = ingestBaseUrl;
      return this;
    }

    public Builder instances(int instances) {
      this.instances = instances;
      return this;
    }

    public Builder maxMemoryBytes(long maxMemoryBytes) {
      this.maxMemoryBytes = maxMemoryBytes;
      return this;
    }

    public Builder interpreter(boolean interpreter) {
      this.interpreter = interpreter;
      return this;
    }

    public Builder exposure(ExposureMode exposure) {
      this.exposure = exposure;
      return this;
    }

    public Builder pollInterval(Duration pollInterval) {
      this.pollInterval = pollInterval;
      return this;
    }

    public Builder flushInterval(Duration flushInterval) {
      this.flushInterval = flushInterval;
      return this;
    }

    public Builder retryInterval(Duration retryInterval) {
      this.retryInterval = retryInterval;
      return this;
    }

    public Builder logger(Logger logger) {
      this.logger = logger;
      return this;
    }

    Builder applyFilter(Filter applyFilter) {
      this.applyFilter = applyFilter;
      return this;
    }

    public Builder onReady(Runnable onReady) {
      this.onReady = onReady;
      return this;
    }

    public EngineConfig build() {
      if (clientSecret == null || encryptionKey == null) {
        throw new IllegalArgumentException("clientSecret and encryptionKey are required");
      }
      if (pollInterval == null || flushInterval == null || retryInterval == null) {
        throw new IllegalArgumentException("intervals are required");
      }
      return new EngineConfig(this);
    }
  }
}
