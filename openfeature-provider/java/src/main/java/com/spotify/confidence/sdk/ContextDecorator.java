package com.spotify.confidence.sdk;

import dev.openfeature.sdk.MutableContext;

/**
 * Decorates the evaluation context with additional data from the request. Use cases include adding
 * user ID from auth headers, adding request metadata, etc.
 *
 * <p>Example:
 *
 * <pre>{@code
 * ContextDecorator decorator = (ctx, req) -> {
 *     // Add user ID from auth header
 *     List<String> userIds = req.getHeaders().get("X-User-Id");
 *     if (userIds != null && !userIds.isEmpty()) {
 *         ctx.add("user_id", userIds.get(0));
 *     }
 *
 *     // Add request source
 *     ctx.add("request_source", "backend_proxy");
 * };
 *
 * FlagResolverService service = new FlagResolverService(provider, decorator);
 * }</pre>
 */
@FunctionalInterface
@Experimental
public interface ContextDecorator {

  /**
   * Decorates the evaluation context with additional data from the request.
   *
   * @param context the mutable evaluation context to decorate
   * @param request the incoming HTTP request containing headers and other metadata
   */
  void decorate(MutableContext context, FlagResolverRequest request);
}
