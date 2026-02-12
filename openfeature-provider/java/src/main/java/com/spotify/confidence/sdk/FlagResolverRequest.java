package com.spotify.confidence.sdk;

import java.io.InputStream;
import java.util.List;
import java.util.Map;

/**
 * Represents an incoming HTTP request to the flag resolver service. Framework adapters (Spring,
 * Javalin, etc.) should implement this interface to bridge their HTTP request objects.
 *
 * <p>Example implementation for Javalin:
 *
 * <pre>{@code
 * class JavalinFlagResolverRequest implements FlagResolverRequest {
 *     private final Context ctx;
 *
 *     JavalinFlagResolverRequest(Context ctx) { this.ctx = ctx; }
 *
 *     @Override
 *     public String getMethod() { return ctx.method().name(); }
 *
 *     @Override
 *     public InputStream getBody() { return ctx.bodyInputStream(); }
 *
 *     @Override
 *     public Map<String, List<String>> getHeaders() { return ctx.headerMap(); }
 * }
 * }</pre>
 */
@Experimental
public interface FlagResolverRequest {

  /** Returns the HTTP method (GET, POST, etc.) */
  String getMethod();

  /** Returns the request body as an InputStream */
  InputStream getBody();

  /**
   * Returns request headers. Used by {@link ContextDecorator} to add additional context from
   * headers (e.g., user ID from auth headers).
   *
   * @return a map of header names to their values
   */
  Map<String, List<String>> getHeaders();
}
