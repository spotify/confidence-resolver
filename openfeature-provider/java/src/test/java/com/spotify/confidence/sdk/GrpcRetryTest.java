package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.spotify.confidence.sdk.flags.resolver.v1.*;
import io.grpc.ClientInterceptor;
import io.grpc.Server;
import io.grpc.Status;
import io.grpc.inprocess.InProcessChannelBuilder;
import io.grpc.inprocess.InProcessServerBuilder;
import io.grpc.stub.StreamObserver;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;

class GrpcRetryTest {

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

    Server server =
        InProcessServerBuilder.forName(serverName)
            .directExecutor()
            .addService(service)
            .build()
            .start();

    try {
      final ChannelFactory retryChannelFactory =
          (target, interceptors) -> {
            InProcessChannelBuilder builder =
                InProcessChannelBuilder.forName(serverName)
                    .directExecutor()
                    .defaultServiceConfig(DefaultChannelFactory.RETRY_SERVICE_CONFIG)
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

      assertTrue(
          successLatch.await(5, TimeUnit.SECONDS), "Write should have succeeded after retry");
      assertEquals(2, callCount.get(), "Server should have received 2 calls (1 fail + 1 retry)");
    } finally {
      server.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
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

    Server server =
        InProcessServerBuilder.forName(serverName)
            .directExecutor()
            .addService(service)
            .build()
            .start();

    try {
      final ChannelFactory retryChannelFactory =
          (target, interceptors) -> {
            InProcessChannelBuilder builder =
                InProcessChannelBuilder.forName(serverName)
                    .directExecutor()
                    .defaultServiceConfig(DefaultChannelFactory.RETRY_SERVICE_CONFIG)
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
    } finally {
      server.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
  }

  @Test
  void allRetriesExhausted() throws Exception {
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
                Status.UNAVAILABLE.withDescription("always fails").asRuntimeException());
          }
        };

    Server server =
        InProcessServerBuilder.forName(serverName)
            .directExecutor()
            .addService(service)
            .build()
            .start();

    try {
      final ChannelFactory retryChannelFactory =
          (target, interceptors) -> {
            InProcessChannelBuilder builder =
                InProcessChannelBuilder.forName(serverName)
                    .directExecutor()
                    .defaultServiceConfig(DefaultChannelFactory.RETRY_SERVICE_CONFIG)
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

      assertEquals(3, callCount.get(), "Should have retried exactly 3 times (maxAttempts)");
    } finally {
      server.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
  }

  @Test
  void shutdownCompletesWhenRetriesExhausted() throws Exception {
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
                Status.UNAVAILABLE.withDescription("always fails").asRuntimeException());
          }
        };

    Server server =
        InProcessServerBuilder.forName(serverName)
            .directExecutor()
            .addService(service)
            .build()
            .start();

    try {
      final ChannelFactory retryChannelFactory =
          (target, interceptors) -> {
            InProcessChannelBuilder builder =
                InProcessChannelBuilder.forName(serverName)
                    .directExecutor()
                    .defaultServiceConfig(DefaultChannelFactory.RETRY_SERVICE_CONFIG)
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

      long start = System.currentTimeMillis();
      logger.shutdown();
      long duration = System.currentTimeMillis() - start;

      assertTrue(duration < 30_000, "Shutdown should complete within 30s, took " + duration + "ms");
      assertTrue(callCount.get() > 0, "Server should have received at least one call");
    } finally {
      server.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
  }

  @Test
  void customChannelFactoryWithRetryConfig() throws Exception {
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

    Server server =
        InProcessServerBuilder.forName(serverName)
            .directExecutor()
            .addService(service)
            .build()
            .start();

    try {
      final ChannelFactory customFactory =
          (target, interceptors) -> {
            InProcessChannelBuilder builder =
                InProcessChannelBuilder.forName(serverName)
                    .directExecutor()
                    .defaultServiceConfig(DefaultChannelFactory.RETRY_SERVICE_CONFIG)
                    .enableRetry();
            if (!interceptors.isEmpty()) {
              builder.intercept(interceptors.toArray(new ClientInterceptor[0]));
            }
            return builder.build();
          };

      final var logger = new GrpcWasmFlagLogger("test-secret", customFactory);

      final var request =
          WriteFlagLogsRequest.newBuilder()
              .addFlagAssigned(FlagAssigned.newBuilder().setResolveId("r1").build())
              .build();

      logger.write(request);
      logger.shutdown();

      assertTrue(
          successLatch.await(5, TimeUnit.SECONDS),
          "Custom factory with retry config should enable retries");
      assertEquals(2, callCount.get(), "Should have retried once (2 total calls)");
    } finally {
      server.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
  }

  @Test
  void multipleWritesThenShutdown() throws Exception {
    final var serverName = InProcessServerBuilder.generateName();
    final var callCount = new AtomicInteger(0);

    final var service =
        new InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceImplBase() {
          @Override
          public void clientWriteFlagLogs(
              WriteFlagLogsRequest request,
              StreamObserver<WriteFlagLogsResponse> responseObserver) {
            callCount.incrementAndGet();
            responseObserver.onNext(WriteFlagLogsResponse.getDefaultInstance());
            responseObserver.onCompleted();
          }
        };

    Server server =
        InProcessServerBuilder.forName(serverName)
            .directExecutor()
            .addService(service)
            .build()
            .start();

    try {
      final ChannelFactory retryChannelFactory =
          (target, interceptors) -> {
            InProcessChannelBuilder builder =
                InProcessChannelBuilder.forName(serverName)
                    .directExecutor()
                    .defaultServiceConfig(DefaultChannelFactory.RETRY_SERVICE_CONFIG)
                    .enableRetry();
            if (!interceptors.isEmpty()) {
              builder.intercept(interceptors.toArray(new ClientInterceptor[0]));
            }
            return builder.build();
          };

      final var logger = new GrpcWasmFlagLogger("test-secret", retryChannelFactory);

      for (int i = 0; i < 5; i++) {
        final var request =
            WriteFlagLogsRequest.newBuilder()
                .addFlagAssigned(FlagAssigned.newBuilder().setResolveId("r" + i).build())
                .build();
        logger.write(request);
      }

      logger.shutdown();

      assertEquals(5, callCount.get(), "All 5 writes should complete with retry config active");
    } finally {
      server.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
  }
}
