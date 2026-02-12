package com.spotify.confidence.sdk;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.Map;

/** Default implementation of {@link FlagResolverResponse} using byte arrays for the body. */
public final class DefaultFlagResolverResponse implements FlagResolverResponse {
  private final int statusCode;
  private final byte[] body;
  private final Map<String, String> headers;

  private DefaultFlagResolverResponse(int statusCode, byte[] body, Map<String, String> headers) {
    this.statusCode = statusCode;
    this.body = body;
    this.headers = headers;
  }

  @Override
  public int getStatusCode() {
    return statusCode;
  }

  @Override
  public InputStream getBody() {
    return new ByteArrayInputStream(body);
  }

  @Override
  public Map<String, String> getHeaders() {
    return headers;
  }

  @Override
  public String getBodyAsString() {
    return new String(body, StandardCharsets.UTF_8);
  }

  /**
   * Creates a successful response with the given body.
   *
   * @param body the response body as an InputStream
   * @return a 200 OK response
   * @throws IOException if reading the body fails
   */
  public static FlagResolverResponse ok(InputStream body) throws IOException {
    return new DefaultFlagResolverResponse(
        200, body.readAllBytes(), Map.of("Content-Type", "application/json"));
  }

  /**
   * Creates a successful response with the given JSON body.
   *
   * @param jsonBody the response body as a JSON string
   * @return a 200 OK response
   */
  public static FlagResolverResponse ok(String jsonBody) {
    return new DefaultFlagResolverResponse(
        200, jsonBody.getBytes(StandardCharsets.UTF_8), Map.of("Content-Type", "application/json"));
  }

  /**
   * Creates an error response with the given status code and message.
   *
   * @param statusCode the HTTP status code
   * @param message the error message
   * @return an error response
   */
  public static FlagResolverResponse error(int statusCode, String message) {
    String escapedMessage = message.replace("\\", "\\\\").replace("\"", "\\\"");
    String jsonError = "{\"error\":\"" + escapedMessage + "\"}";
    return new DefaultFlagResolverResponse(
        statusCode,
        jsonError.getBytes(StandardCharsets.UTF_8),
        Map.of("Content-Type", "application/json"));
  }
}
