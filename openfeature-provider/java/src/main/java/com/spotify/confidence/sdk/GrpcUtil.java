package com.spotify.confidence.sdk;

import com.google.common.util.concurrent.FutureCallback;
import com.google.common.util.concurrent.Futures;
import com.google.common.util.concurrent.ListenableFuture;
import com.google.common.util.concurrent.MoreExecutors;
import io.grpc.ManagedChannel;
import java.time.Duration;
import java.util.List;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;

/**
 * Utility class for converting gRPC ListenableFuture to Java CompletableFuture. Copied from the
 * main SDK to avoid dependencies.
 */
final class GrpcUtil {

  private static final String CONFIDENCE_DOMAIN = "edge-grpc.spotify.com";

  private GrpcUtil() {}

  static <T> CompletableFuture<T> toCompletableFuture(final ListenableFuture<T> listenableFuture) {
    final CompletableFuture<T> completableFuture =
        new CompletableFuture<>() {
          @Override
          public boolean cancel(boolean mayInterruptIfRunning) {
            listenableFuture.cancel(mayInterruptIfRunning);
            return super.cancel(mayInterruptIfRunning);
          }
        };
    Futures.addCallback(
        listenableFuture,
        new FutureCallback<T>() {
          @Override
          public void onSuccess(T result) {
            completableFuture.complete(result);
          }

          @Override
          public void onFailure(Throwable t) {
            completableFuture.completeExceptionally(t);
          }
        },
        MoreExecutors.directExecutor());
    return completableFuture;
  }

  static ManagedChannel createConfidenceChannel(ChannelFactory channelFactory) {
    final String confidenceDomain =
        Optional.ofNullable(System.getenv("CONFIDENCE_DOMAIN")).orElse(CONFIDENCE_DOMAIN);
    return channelFactory.create(
        confidenceDomain, List.of(new DefaultDeadlineClientInterceptor(Duration.ofMinutes(1))));
  }

  /**
   * Creates a channel to the Confidence events service (confidence.events.v1.EventsService).
   *
   * <p>This is deliberately a separate channel from {@link
   * #createConfidenceChannel(ChannelFactory)} even though it resolves to the same host by default.
   * The two carry different default deadlines (30s for event publishing vs 1min for flag log
   * ingestion), their targets can be overridden independently via {@code CONFIDENCE_EVENTS_DOMAIN}
   * / {@code CONFIDENCE_DOMAIN}, and their lifecycles are independent: the flag-log channel is
   * owned and shut down by {@link GrpcWasmFlagLogger}, while the events channel is owned by the
   * provider and must stay open until the final event drain during shutdown completes.
   *
   * <p>Retries for transient {@code UNAVAILABLE} failures on {@code
   * confidence.events.v1.EventsService} come from {@link
   * DefaultChannelFactory#RETRY_SERVICE_CONFIG}, which is installed on the {@code
   * ManagedChannelBuilder} via {@code defaultServiceConfig} + {@code enableRetry}. gRPC only
   * accepts a service config at build time, so a caller-supplied {@link ChannelFactory} is
   * responsible for configuring its own retries.
   */
  static ManagedChannel createConfidenceEventsChannel(ChannelFactory channelFactory) {
    final String eventsDomain =
        Optional.ofNullable(System.getenv("CONFIDENCE_EVENTS_DOMAIN"))
            .or(() -> Optional.ofNullable(System.getenv("CONFIDENCE_DOMAIN")))
            .orElse(CONFIDENCE_DOMAIN);
    return channelFactory.create(
        eventsDomain, List.of(new DefaultDeadlineClientInterceptor(Duration.ofSeconds(30))));
  }
}
