package com.spotify.confidence.sdk;

import java.net.http.HttpClient;
import java.time.Duration;

/**
 * Advanced/testing hook supplying the {@link HttpClient} the provider uses for every network call:
 * module downloads from the CDN, flag-log delivery, and the hosted {@link
 * RemoteMaterializationStore}.
 *
 * <p>The provider calls {@link #createHttpClient()} once and shares the client across those
 * components for the provider's lifetime. The client must use {@link HttpClient.Redirect#NEVER} for
 * module profile registration. The provider won't send the client secret with a client that follows
 * redirects. The factory owns the lifecycle of the client it creates.
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

  /** The default factory: a client with a 10-second connect timeout and redirects disabled. */
  static HttpClientFactory defaultFactory() {
    return () ->
        HttpClient.newBuilder()
            .connectTimeout(Duration.ofSeconds(10))
            .followRedirects(HttpClient.Redirect.NEVER)
            .build();
  }
}
