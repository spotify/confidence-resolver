package com.spotify.confidence.sdk;

import java.io.IOException;
import java.net.HttpURLConnection;

/**
 * HttpClientFactory is an advanced/testing hook allowing callers to customize how HTTP connections
 * are created. The provider will pass the URL that needs to be fetched.
 *
 * <p>Implementations may modify request properties, change URLs, or replace the connection creation
 * mechanism entirely. This is particularly useful for:
 *
 * <ul>
 *   <li>Unit testing: inject mock HTTP responses
 *   <li>Integration testing: point to local mock HTTP servers
 *   <li>Production customization: custom timeouts, proxies, headers
 *   <li>Debugging: add custom logging or request tracking
 * </ul>
 *
 * <p><strong>Lifecycle:</strong> The factory is responsible for managing any resources it creates.
 * When {@link #shutdown()} is called, it should clean up any resources that were allocated.
 */
public interface HttpClientFactory {
  /**
   * Creates an HTTP connection for the given URL.
   *
   * @param url the URL to connect to (e.g.,
   *     "https://confidence-resolver-state-cdn.spotifycdn.com/...")
   * @return a configured HttpURLConnection
   * @throws IOException if an I/O error occurs while opening the connection
   */
  HttpURLConnection create(String url) throws IOException;

  /**
   * Shuts down this factory and cleans up any resources. This method should be called when the
   * provider is shutting down to ensure proper resource cleanup.
   *
   * <p>Implementations should clean up any resources that were created and wait for them to
   * terminate gracefully if applicable.
   */
  void shutdown();
}
