package com.spotify.confidence.sdk;

import java.util.Map;

/**
 * Represents an HTTP response from the flag resolver service.
 *
 * <p>Example usage in Javalin:
 *
 * <pre>{@code
 * app.post("/v1/flags:resolve", ctx -> {
 *     ConfidenceHttpRequest request = new JavalinConfidenceHttpRequest(ctx);
 *     ConfidenceHttpResponse response = flagResolver.handleResolve(request);
 *     ctx.status(response.getStatusCode())
 *        .contentType("application/json")
 *        .result(response.getBody());
 * });
 * }</pre>
 */
@Experimental
public interface ConfidenceHttpResponse {

  /** Returns the HTTP status code for this response. */
  int getStatusCode();

  /** Returns the response body as a byte array. May be null for error responses. */
  byte[] getBody();

  /** Returns the response headers. */
  Map<String, String> getHeaders();
}
