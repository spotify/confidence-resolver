package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.spotify.confidence.sdk.flags.resolver.v1.*;
import io.grpc.ClientInterceptor;
import io.grpc.ManagedChannel;
import io.grpc.Status;
import io.grpc.inprocess.InProcessChannelBuilder;
import io.grpc.inprocess.InProcessServerBuilder;
import io.grpc.stub.StreamObserver;
import io.grpc.testing.GrpcCleanupRule;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;

class GrpcRetryTest {

  private final GrpcCleanupRule grpcCleanup = new GrpcCleanupRule();

  private static final Map<String, Object> RETRY_SERVICE_CONFIG =
      Map.of(
          "methodConfig",
          List.of(
              Map.of(
                  "name",
                  List.of(
                      Map.of(
                          "service",
                          "confidence.flags.resolver.v1.InternalFlagLoggerService")),
                  "retryPolicy",
                  Map.of(
                      "maxAttempts", 3.0,
                      "initialBackoff", "0.1s",
                      "maxBackoff", "0.5s",
                      "backoffMultiplier", 2.0,
                      "retryableStatusCodes", List.of("UNAVAILABLE")))));

  @Test
  void retryOnUnavailable() throws Exception {
    final var serverName = InProcessServerBuilder.generateName();
    final var callCount = new AtomicInteger(0);
    final var successLatch = new CountDownLatch(1);

    final var service =
        new InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceImplBase() {
          @Override
          public void clientWriteFlagLogs(
              WriteFlagLogsRequest request,
              StreamObserver<WriteFlagLogsResponse> responseObserver) {
            int attempt = callCount.incrementAndGet();
            if (attempt == 1) {
              responseObserver.onError(
                  Status.UNAVAILABLE.withDescription("simulated").asRuntimeException());
            } else {
              successLatch.countDown();
              responseObserver.onNext(WriteFlagLogsResponse.getDefaultInstance());
              responseObserver.onCompleted();
            }
          }
        };

    grpcCleanup.register(
        InProcessServerBuilder.forName(serverName)
            .directExecutor()
            .addService(service)
            .build()
            .start());

    final ChannelFactory retryChannelFactory =
        (target, interceptors) -> {
          InProcessChannelBuilder builder =
              InProcessChannelBuilder.forName(serverName)
                  .directExecutor()
                  .defaultServiceConfig(RETRY_SERVICE_CONFIG)
                  .enableRetry();
          if (!interceptors.isEmpty()) {
            builder.intercept(interceptors.toArray(new ClientInterceptor[0]));
          }
          return builder.build();
        };

    final var logger = new GrpcWasmFlagLogger("test-secret", retryChannelFactory);

    final var request =
        WriteFlagLogsRequest.newBuilder()
            .addFlagAssigned(FlagAssigned.newBuilder().setResolveId("r1").build())
            .build();

    logger.write(request);
    logger.shutdown();

    assertTrue(successLatch.await(5, TimeUnit.SECONDS), "Write should have succeeded after retry");
    assertEquals(2, callCount.get(), "Server should have received 2 calls (1 fail + 1 retry)");

    grpcCleanup.after();
  }

  @Test
  void noRetryOnPermissionDenied() throws Exception {
    final var serverName = InProcessServerBuilder.generateName();
    final var callCount = new AtomicInteger(0);

    final var service =
        new InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceImplBase() {
          @Override
          public void clientWriteFlagLogs(
              WriteFlagLogsRequest request,
              StreamObserver<WriteFlagLogsResponse> responseObserver) {
            callCount.incrementAndGet();
            responseObserver.onError(
                Status.PERMISSION_DENIED.withDescription("not retryable").asRuntimeException());
          }
        };

    grpcCleanup.register(
        InProcessServerBuilder.forName(serverName)
            .directExecutor()
            .addService(service)
            .build()
            .start());

    final ChannelFactory retryChannelFactory =
        (target, interceptors) -> {
          InProcessChannelBuilder builder =
              InProcessChannelBuilder.forName(serverName)
                  .directExecutor()
                  .defaultServiceConfig(RETRY_SERVICE_CONFIG)
                  .enableRetry();
          if (!interceptors.isEmpty()) {
            builder.intercept(interceptors.toArray(new ClientInterceptor[0]));
          }
          return builder.build();
        };

    final var logger = new GrpcWasmFlagLogger("test-secret", retryChannelFactory);

    final var request =
        WriteFlagLogsRequest.newBuilder()
            .addFlagAssigned(FlagAssigned.newBuilder().setResolveId("r1").build())
            .build();

    logger.write(request);
    logger.shutdown();

    assertEquals(1, callCount.get(), "PERMISSION_DENIED should not trigger retry");

    grpcCleanup.after();
  }
}
