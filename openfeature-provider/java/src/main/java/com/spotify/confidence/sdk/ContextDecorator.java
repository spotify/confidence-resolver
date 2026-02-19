package com.spotify.confidence.sdk;

import dev.openfeature.sdk.EvaluationContext;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.function.BiFunction;

/**
 * Decorates the evaluation context with additional data from the request. Use cases include adding
 * user ID from auth headers, adding request metadata, etc.
 *
 * <p>The type parameter {@code R} allows decorators to work with richer request types that extend
 * {@link ConfidenceHttpRequest} with additional framework-specific methods.
 *
 * <p>For synchronous decorators, use the {@link #sync(BiFunction)} factory method:
 *
 * <pre>{@code
 * ContextDecorator<ConfidenceHttpRequest> decorator = ContextDecorator.sync((ctx, req) -> {
 *     List<String> userIds = req.headers().get("X-User-Id");
 *     if (userIds != null && !userIds.isEmpty()) {
 *         return ctx.merge(new ImmutableContext(userIds.get(0)));
 *     }
 *     return ctx;
 * });
 * }</pre>
 *
 * @param <R> the request type, must extend {@link ConfidenceHttpRequest}
 */
@FunctionalInterface
@Experimental
public interface ContextDecorator<R extends ConfidenceHttpRequest> {

  /**
   * Decorates the evaluation context with additional data from the request. The returned
   * CompletionStage completes with the decorated context, allowing async operations such as
   * fetching data from external services.
   *
   * @param context the evaluation context to decorate
   * @param request the incoming request containing headers and other metadata
   * @return a CompletionStage that completes with the decorated context
   */
  CompletionStage<EvaluationContext> decorate(EvaluationContext context, R request);

  /**
   * Creates a synchronous context decorator from a {@link BiFunction}. The function receives the
   * current context and request, and returns a new decorated context.
   *
   * @param decorator the synchronous decorator
   * @param <R> the request type
   * @return a context decorator that completes immediately
   */
  static <R extends ConfidenceHttpRequest> ContextDecorator<R> sync(
      BiFunction<EvaluationContext, R, EvaluationContext> decorator) {
    return (ctx, req) -> CompletableFuture.completedFuture(decorator.apply(ctx, req));
  }
}
