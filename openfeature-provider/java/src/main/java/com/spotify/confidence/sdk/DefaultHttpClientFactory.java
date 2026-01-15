package com.spotify.confidence.sdk;

import java.io.IOException;
import java.net.HttpURLConnection;
import java.net.URL;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Default implementation of HttpClientFactory that creates standard HTTP connections.
 *
 * <p>This factory:
 *
 * <ul>
 *   <li>Creates HttpURLConnection instances for the given URLs
 *   <li>Sets sensible default timeouts (10s connect, 30s read)
 *   <li>Can be extended or replaced for testing or custom behavior
 * </ul>
 */
public class DefaultHttpClientFactory implements HttpClientFactory {
  private static final Logger logger = LoggerFactory.getLogger(DefaultHttpClientFactory.class);

  /** Default connection timeout in milliseconds (10 seconds) */
  private static final int DEFAULT_CONNECT_TIMEOUT_MS = 10_000;

  /** Default read timeout in milliseconds (30 seconds) */
  private static final int DEFAULT_READ_TIMEOUT_MS = 30_000;

  @Override
  public HttpURLConnection create(String url) throws IOException {
    HttpURLConnection connection = (HttpURLConnection) new URL(url).openConnection();
    connection.setConnectTimeout(DEFAULT_CONNECT_TIMEOUT_MS);
    connection.setReadTimeout(DEFAULT_READ_TIMEOUT_MS);
    return connection;
  }

  @Override
  public void shutdown() {
    // HTTP connections are stateless and don't require cleanup
    logger.debug("DefaultHttpClientFactory shutdown called (no-op for stateless HTTP)");
  }
}
