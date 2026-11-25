package com.spotify.confidence.sdk;

import java.net.http.HttpClient;
import java.time.Duration;

/**
 * Advanced/testing hook supplying the {@link HttpClient} the provider uses for every network call:
 * module downloads from the CDN, flag-log delivery, and the hosted {@link
 * RemoteMaterializationStore}.
 *
 * <p>The provider calls {@link #createHttpClient()} once and shares the client across those
 * components for the provider's lifetime. If the client follows redirects, authenticated module
 * profile registration uses a separate client with the same transport settings and redirects
 * disabled. The factory itself owns the lifecycle of whatever it creates; {@code
 * java.net.http.HttpClient} needs no explicit shutdown.
 *
 * <p>Useful for:
 *
 * <ul>
 *   <li>Unit testing: inject clients wired to local mock servers
 *   <li>Production customization: proxies, custom timeouts
 * </ul>
 */
@FunctionalInterface
public interface HttpClientFactory {

  /**
   * Creates the HTTP client the provider will use for all its network traffic.
   *
   * @return a configured {@link HttpClient}
   */
  HttpClient createHttpClient();

  /**
   * The default factory: a client with a 10-second connect timeout that follows redirects, as the
   * module CDN and ingestion endpoints may answer with one.
   */
  static HttpClientFactory defaultFactory() {
    return () ->
        HttpClient.newBuilder()
            .connectTimeout(Duration.ofSeconds(10))
            .followRedirects(HttpClient.Redirect.NORMAL)
            .build();
  }
}
