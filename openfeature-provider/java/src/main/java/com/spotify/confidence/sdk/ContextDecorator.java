package com.spotify.confidence.sdk;

import dev.openfeature.sdk.MutableContext;
import java.util.concurrent.CompletionStage;

/**
 * Decorates the evaluation context with additional data from the request. Use cases include adding
 * user ID from auth headers, adding request metadata, etc.
 *
 * <p>Example:
 *
 * <pre>{@code
 * ContextDecorator decorator = (ctx, req) -> {
 *     // Add user ID from auth header
 *     List<String> userIds = req.headers().get("X-User-Id");
 *     if (userIds != null && !userIds.isEmpty()) {
 *         ctx.add("user_id", userIds.get(0));
 *     }
 *
 *     // Add request source
 *     ctx.add("request_source", "backend_proxy");
 *     return CompletableFuture.completedFuture(null);
 * };
 *
 * FlagResolverService service = new FlagResolverService(provider, decorator);
 * }</pre>
 */
@FunctionalInterface
@Experimental
public interface ContextDecorator {

  /**
   * Decorates the evaluation context with additional data from the request. The returned
   * CompletionStage completes when the decoration is done, allowing async operations such as
   * fetching data from external services.
   *
   * @param context the mutable evaluation context to decorate
   * @param request the incoming HTTP request containing headers and other metadata
   * @return a CompletionStage that completes when decoration is done
   */
  CompletionStage<Void> decorate(MutableContext context, ConfidenceHttpRequest request);
}
