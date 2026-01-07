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
 *   <li>Uses default timeouts and settings from the JVM
 *   <li>Can be extended or replaced for testing or custom behavior
 * </ul>
 */
public class DefaultHttpClientFactory implements HttpClientFactory {
  private static final Logger logger = LoggerFactory.getLogger(DefaultHttpClientFactory.class);

  @Override
  public HttpURLConnection create(String url) throws IOException {
    return (HttpURLConnection) new URL(url).openConnection();
  }

  @Override
  public void shutdown() {
    // HTTP connections are stateless and don't require cleanup
    logger.debug("DefaultHttpClientFactory shutdown called (no-op for stateless HTTP)");
  }
}
