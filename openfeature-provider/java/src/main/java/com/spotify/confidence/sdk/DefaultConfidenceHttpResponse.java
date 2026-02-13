package com.spotify.confidence.sdk;

import java.nio.charset.StandardCharsets;
import java.util.Map;

/** Default implementation of {@link ConfidenceHttpResponse} using byte arrays for the body. */
final class DefaultConfidenceHttpResponse implements ConfidenceHttpResponse {
  private final int statusCode;
  private final byte[] body;
  private final Map<String, String> headers;

  private DefaultConfidenceHttpResponse(int statusCode, byte[] body, Map<String, String> headers) {
    this.statusCode = statusCode;
    this.body = body;
    this.headers = headers;
  }

  @Override
  public int getStatusCode() {
    return statusCode;
  }

  @Override
  public byte[] getBody() {
    return body;
  }

  @Override
  public Map<String, String> getHeaders() {
    return headers;
  }

  /**
   * Creates a successful response with the given body.
   *
   * @param body the response body as a byte array
   * @return a 200 OK response with application/json content type
   */
  public static ConfidenceHttpResponse ok(byte[] body) {
    return new DefaultConfidenceHttpResponse(200, body, Map.of("Content-Type", "application/json"));
  }

  /**
   * Creates a successful response with the given JSON body.
   *
   * @param jsonBody the response body as a JSON string
   * @return a 200 OK response with application/json content type
   */
  public static ConfidenceHttpResponse ok(String jsonBody) {
    return new DefaultConfidenceHttpResponse(
        200, jsonBody.getBytes(StandardCharsets.UTF_8), Map.of("Content-Type", "application/json"));
  }

  /**
   * Creates an error response with the given status code.
   *
   * @param statusCode the HTTP status code
   * @return an error response with the given status code and no body
   */
  public static ConfidenceHttpResponse error(int statusCode) {
    return new DefaultConfidenceHttpResponse(statusCode, null, Map.of());
  }
}
