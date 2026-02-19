package com.spotify.confidence.sdk;

import java.util.List;
import java.util.Map;

/**
 * Represents an incoming HTTP request to the flag resolver service. Framework adapters (Spring,
 * Javalin, etc.) should implement this interface to bridge their HTTP request objects.
 *
 * <p>Example implementation for Javalin:
 *
 * <pre>{@code
 * class JavalinConfidenceHttpRequest implements ConfidenceHttpRequest {
 *     private final Context ctx;
 *
 *     JavalinConfidenceHttpRequest(Context ctx) { this.ctx = ctx; }
 *
 *     @Override
 *     public String method() { return ctx.method().name(); }
 *
 *     @Override
 *     public byte[] body() { return ctx.bodyAsBytes(); }
 *
 *     @Override
 *     public Map<String, List<String>> headers() { return ctx.headerMap(); }
 * }
 * }</pre>
 */
@Experimental
public interface ConfidenceHttpRequest {

  /** Returns the HTTP method (GET, POST, etc.) */
  String method();

  /** Returns the request body as a byte array. */
  byte[] body();

  /**
   * Returns request headers. Used by {@link ContextDecorator} to add additional context from
   * headers (e.g., user ID from auth headers).
   *
   * @return a map of header names to their values
   */
  Map<String, List<String>> headers();
}
