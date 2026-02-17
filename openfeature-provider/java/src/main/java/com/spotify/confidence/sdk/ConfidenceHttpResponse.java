package com.spotify.confidence.sdk;

import java.nio.charset.StandardCharsets;
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
 *     ctx.status(response.statusCode())
 *        .contentType("application/json")
 *        .result(response.body());
 * });
 * }</pre>
 */
@Experimental
public record ConfidenceHttpResponse(int statusCode, byte[] body, Map<String, String> headers) {

  /**
   * Creates a successful response with the given body.
   *
   * @param body the response body as a byte array
   * @return a 200 OK response with application/json content type
   */
  public static ConfidenceHttpResponse ok(byte[] body) {
    return new ConfidenceHttpResponse(200, body, Map.of("Content-Type", "application/json"));
  }

  /**
   * Creates a successful response with the given JSON body.
   *
   * @param jsonBody the response body as a JSON string
   * @return a 200 OK response with application/json content type
   */
  public static ConfidenceHttpResponse ok(String jsonBody) {
    return new ConfidenceHttpResponse(
        200, jsonBody.getBytes(StandardCharsets.UTF_8), Map.of("Content-Type", "application/json"));
  }

  /**
   * Creates an error response with the given status code.
   *
   * @param statusCode the HTTP status code
   * @return an error response with the given status code and no body
   */
  public static ConfidenceHttpResponse error(int statusCode) {
    return new ConfidenceHttpResponse(statusCode, null, Map.of());
  }
}
