package com.spotify.confidence.sdk;

import java.io.IOException;
import java.io.InputStream;
import java.util.Map;

/**
 * Represents an HTTP response from the flag resolver service. Use the static factory methods to
 * create responses.
 *
 * <p>Example usage in Javalin:
 *
 * <pre>{@code
 * app.post("/v1/flags:resolve", ctx -> {
 *     FlagResolverRequest request = new JavalinFlagResolverRequest(ctx);
 *     FlagResolverResponse response = flagResolver.handleResolve(request);
 *     ctx.status(response.getStatusCode())
 *        .contentType("application/json")
 *        .result(response.getBody());
 * });
 * }</pre>
 */
@Experimental
public interface FlagResolverResponse {

  /** Returns the HTTP status code for this response. */
  int getStatusCode();

  /** Returns the response body as an InputStream. */
  InputStream getBody();

  /** Returns the response headers. */
  Map<String, String> getHeaders();

  /**
   * Convenience method to get body as a String. Note: this reads and caches the body, so it can
   * only be called once per response.
   *
   * @return the response body as a UTF-8 string
   * @throws IOException if reading the body fails
   */
  String getBodyAsString() throws IOException;

  /**
   * Creates a successful response with the given body.
   *
   * @param body the response body as an InputStream
   * @return a 200 OK response
   * @throws IOException if reading the body fails
   */
  static FlagResolverResponse ok(InputStream body) throws IOException {
    return DefaultFlagResolverResponse.ok(body);
  }

  /**
   * Creates a successful response with the given JSON body.
   *
   * @param jsonBody the response body as a JSON string
   * @return a 200 OK response
   */
  static FlagResolverResponse ok(String jsonBody) {
    return DefaultFlagResolverResponse.ok(jsonBody);
  }

  /**
   * Creates an error response with the given status code and message.
   *
   * @param statusCode the HTTP status code
   * @param message the error message
   * @return an error response
   */
  static FlagResolverResponse error(int statusCode, String message) {
    return DefaultFlagResolverResponse.error(statusCode, message);
  }
}
