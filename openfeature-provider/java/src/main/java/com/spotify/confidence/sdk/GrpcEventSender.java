package com.spotify.confidence.sdk;

import static com.spotify.confidence.sdk.GrpcUtil.createConfidenceChannel;

import com.google.protobuf.Timestamp;
import com.spotify.confidence.sdk.events.v1.EventsServiceGrpc;
import com.spotify.confidence.sdk.events.v1.PublishEventsRequest;
import com.spotify.confidence.sdk.wasm.Messages;
import io.grpc.CallOptions;
import io.grpc.Channel;
import io.grpc.ClientCall;
import io.grpc.ClientInterceptor;
import io.grpc.ForwardingClientCall;
import io.grpc.ManagedChannel;
import io.grpc.Metadata;
import io.grpc.MethodDescriptor;
import java.time.Duration;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

class GrpcEventSender implements Consumer<Messages.FlushEventsResponse> {
  private static final Logger logger = LoggerFactory.getLogger(GrpcEventSender.class);
  private static final Duration DEFAULT_SHUTDOWN_TIMEOUT = Duration.ofSeconds(10);

  private final String clientSecret;
  private final EventsServiceGrpc.EventsServiceBlockingStub stub;
  private final ExecutorService executorService;
  private final Duration shutdownTimeout;
  private final ManagedChannel channel;

  GrpcEventSender(String clientSecret, ChannelFactory channelFactory) {
    this.clientSecret = clientSecret;
    this.channel = createConfidenceChannel(channelFactory);
    this.stub = addAuthInterceptor(EventsServiceGrpc.newBlockingStub(channel), clientSecret);
    this.executorService = Executors.newCachedThreadPool();
    this.shutdownTimeout = DEFAULT_SHUTDOWN_TIMEOUT;
  }

  @Override
  public void accept(Messages.FlushEventsResponse response) {
    final PublishEventsRequest.Builder builder =
        PublishEventsRequest.newBuilder().setClientSecret(clientSecret);

    for (Messages.Event wasmEvent : response.getEventsList()) {
      builder.addEvents(
          com.spotify.confidence.sdk.events.v1.Event.newBuilder()
              .setEventDefinition(wasmEvent.getEventDefinition())
              .setPayload(wasmEvent.getPayload())
              .setEventTime(wasmEvent.getEventTime())
              .build());
    }

    java.time.Instant now = java.time.Instant.now();
    builder.setSendTime(
        Timestamp.newBuilder().setSeconds(now.getEpochSecond()).setNanos(now.getNano()).build());

    final PublishEventsRequest request = builder.build();

    executorService.submit(
        () -> {
          try {
            stub.publishEvents(request);
            logger.debug("Successfully published {} events", response.getEventsCount());
          } catch (Exception e) {
            logger.error("Failed to publish events", e);
          }
        });
  }

  void shutdown() {
    executorService.shutdown();
    try {
      if (!executorService.awaitTermination(
          shutdownTimeout.toMillis(), TimeUnit.MILLISECONDS)) {
        logger.warn("Event sender executor did not terminate gracefully");
        executorService.shutdownNow();
      }
    } catch (InterruptedException e) {
      logger.warn("Interrupted while waiting for event sender shutdown", e);
      executorService.shutdownNow();
      Thread.currentThread().interrupt();
    }

    if (channel != null) {
      channel.shutdown();
      try {
        if (!channel.awaitTermination(shutdownTimeout.toMillis(), TimeUnit.MILLISECONDS)) {
          channel.shutdownNow();
        }
      } catch (InterruptedException e) {
        channel.shutdownNow();
        Thread.currentThread().interrupt();
      }
    }
  }

  private static EventsServiceGrpc.EventsServiceBlockingStub addAuthInterceptor(
      EventsServiceGrpc.EventsServiceBlockingStub stub, String clientSecret) {
    return stub.withInterceptors(
        new ClientInterceptor() {
          @Override
          public <ReqT, RespT> ClientCall<ReqT, RespT> interceptCall(
              MethodDescriptor<ReqT, RespT> method, CallOptions callOptions, Channel next) {
            return new ForwardingClientCall.SimpleForwardingClientCall<ReqT, RespT>(
                next.newCall(method, callOptions)) {
              @Override
              public void start(Listener<RespT> responseListener, Metadata headers) {
                Metadata.Key<String> authKey =
                    Metadata.Key.of("authorization", Metadata.ASCII_STRING_MARSHALLER);
                headers.put(authKey, "ClientSecret " + clientSecret);
                super.start(responseListener, headers);
              }
            };
          }
        });
  }
}
