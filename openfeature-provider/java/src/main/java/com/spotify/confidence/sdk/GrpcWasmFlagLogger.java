package com.spotify.confidence.sdk;

import static com.spotify.confidence.sdk.GrpcUtil.createConfidenceChannel;

import com.google.common.annotations.VisibleForTesting;
import com.spotify.confidence.sdk.flags.resolver.v1.InternalFlagLoggerServiceGrpc;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import io.grpc.*;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

@FunctionalInterface
interface FlagLogWriter {
  void write(WriteFlagLogsRequest request);
}

public class GrpcWasmFlagLogger implements WasmFlagLogger {
  private static final Logger logger = LoggerFactory.getLogger(GrpcWasmFlagLogger.class);
  // Max number of flag_assigned entries per chunk to avoid exceeding gRPC max message size
  private static final int MAX_FLAG_ASSIGNED_PER_CHUNK = 1000;
  private static final Duration DEFAULT_SHUTDOWN_TIMEOUT = Duration.ofSeconds(10);
  private final InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub stub;
  private final ExecutorService executorService;
  private final FlagLogWriter writer;
  private final Duration shutdownTimeout;
  private ManagedChannel channel;

  @VisibleForTesting
  public GrpcWasmFlagLogger(String clientSecret, FlagLogWriter writer) {
    this.stub = createAuthStub(new DefaultChannelFactory(), clientSecret);
    this.executorService = Executors.newCachedThreadPool();
    this.writer = writer;
    this.shutdownTimeout = DEFAULT_SHUTDOWN_TIMEOUT;
  }

  @VisibleForTesting
  public GrpcWasmFlagLogger(String clientSecret, FlagLogWriter writer, Duration shutdownTimeout) {
    this.stub = createAuthStub(new DefaultChannelFactory(), clientSecret);
    this.executorService = Executors.newCachedThreadPool();
    this.writer = writer;
    this.shutdownTimeout = shutdownTimeout;
  }

  public GrpcWasmFlagLogger(String clientSecret, ChannelFactory channelFactory) {
    this.stub = createAuthStub(channelFactory, clientSecret);
    this.executorService = Executors.newCachedThreadPool();
    this.shutdownTimeout = DEFAULT_SHUTDOWN_TIMEOUT;
    this.writer =
        request ->
            executorService.submit(
                () -> {
                  try {
                    // Debug: Print the payload being sent
                    System.out.println("\n=== WriteFlagLogsRequest Payload (async) ===");
                    System.out.println("Flag assigned count: " + request.getFlagAssignedCount());
                    System.out.println("Client resolve info count: " + request.getClientResolveInfoCount());
                    System.out.println("Flag resolve info count: " + request.getFlagResolveInfoCount());
                    System.out.println("Has telemetry data: " + request.hasTelemetryData());
                    if (request.hasTelemetryData()) {
                      var telemetry = request.getTelemetryData();
                      System.out.println("  SDK: " + (telemetry.hasSdk() ? telemetry.getSdk().getId() + " v" + telemetry.getSdk().getVersion() : "none"));
                      System.out.println("  Resolve errors count: " + telemetry.getResolveErrorsCount());
                      for (var error : telemetry.getResolveErrorsList()) {
                        System.out.println("    - " + error.getErrorCode() + ": " + error.getCount());
                      }
                    }
                    System.out.println("=== End Payload ===\n");

                    stub.clientWriteFlagLogs(request);
                    logger.debug(
                        "Successfully sent flag log with {} entries",
                        request.getFlagAssignedCount());
                  } catch (Exception e) {
                    logger.error("Failed to write flag logs", e);
                  }
                });
  }

  private InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub createAuthStub(
      ChannelFactory channelFactory, String clientSecret) {
    this.channel = createConfidenceChannel(channelFactory);
    return addAuthInterceptor(InternalFlagLoggerServiceGrpc.newBlockingStub(channel), clientSecret);
  }

  @Override
  public void write(WriteFlagLogsRequest request) {
    if (request.getClientResolveInfoList().isEmpty()
        && request.getFlagAssignedList().isEmpty()
        && request.getFlagResolveInfoList().isEmpty()
        && !hasTelemetryWithErrors(request)) {
      logger.debug("Skipping empty flag log request");
      return;
    }

    final int flagAssignedCount = request.getFlagAssignedCount();

    // If flag_assigned list is small enough, send everything as-is
    if (flagAssignedCount <= MAX_FLAG_ASSIGNED_PER_CHUNK) {
      sendAsync(request);
      return;
    }

    // Split flag_assigned into chunks and send each chunk asynchronously
    logger.debug(
        "Splitting {} flag_assigned entries into chunks of {}",
        flagAssignedCount,
        MAX_FLAG_ASSIGNED_PER_CHUNK);

    final List<WriteFlagLogsRequest> chunks = createFlagAssignedChunks(request);
    for (WriteFlagLogsRequest chunk : chunks) {
      sendAsync(chunk);
    }
  }

  private List<WriteFlagLogsRequest> createFlagAssignedChunks(WriteFlagLogsRequest request) {
    final List<WriteFlagLogsRequest> chunks = new ArrayList<>();
    final int totalFlags = request.getFlagAssignedCount();

    for (int i = 0; i < totalFlags; i += MAX_FLAG_ASSIGNED_PER_CHUNK) {
      final int end = Math.min(i + MAX_FLAG_ASSIGNED_PER_CHUNK, totalFlags);
      final WriteFlagLogsRequest.Builder chunkBuilder =
          WriteFlagLogsRequest.newBuilder()
              .addAllFlagAssigned(request.getFlagAssignedList().subList(i, end));

      // Include telemetry and resolve info only in the first chunk
      if (i == 0) {
        if (request.hasTelemetryData()) {
          chunkBuilder.setTelemetryData(request.getTelemetryData());
        }
        chunkBuilder
            .addAllClientResolveInfo(request.getClientResolveInfoList())
            .addAllFlagResolveInfo(request.getFlagResolveInfoList());
      }

      chunks.add(chunkBuilder.build());
    }

    return chunks;
  }

  private void sendAsync(WriteFlagLogsRequest request) {
    writer.write(request);
  }

  @Override
  public void writeSync(WriteFlagLogsRequest request) {
    if (request.getClientResolveInfoList().isEmpty()
        && request.getFlagAssignedList().isEmpty()
        && request.getFlagResolveInfoList().isEmpty()
        && !hasTelemetryWithErrors(request)) {
      logger.debug("Skipping empty flag log request");
      return;
    }

    final int flagAssignedCount = request.getFlagAssignedCount();

    // If flag_assigned list is small enough, send everything as-is
    if (flagAssignedCount <= MAX_FLAG_ASSIGNED_PER_CHUNK) {
      sendSync(request);
      return;
    }

    // Split flag_assigned into chunks and send each chunk synchronously
    logger.debug(
        "Synchronously splitting {} flag_assigned entries into chunks of {}",
        flagAssignedCount,
        MAX_FLAG_ASSIGNED_PER_CHUNK);

    final List<WriteFlagLogsRequest> chunks = createFlagAssignedChunks(request);
    for (WriteFlagLogsRequest chunk : chunks) {
      sendSync(chunk);
    }
  }

  private void sendSync(WriteFlagLogsRequest request) {
    try {
      // Debug: Print the payload being sent
      System.out.println("\n=== WriteFlagLogsRequest Payload (sync) ===");
      System.out.println("Flag assigned count: " + request.getFlagAssignedCount());
      System.out.println("Client resolve info count: " + request.getClientResolveInfoCount());
      System.out.println("Flag resolve info count: " + request.getFlagResolveInfoCount());
      System.out.println("Has telemetry data: " + request.hasTelemetryData());
      if (request.hasTelemetryData()) {
        var telemetry = request.getTelemetryData();
        System.out.println("  SDK: " + (telemetry.hasSdk() ? telemetry.getSdk().getId() + " v" + telemetry.getSdk().getVersion() : "none"));
        System.out.println("  Resolve errors count: " + telemetry.getResolveErrorsCount());
        for (var error : telemetry.getResolveErrorsList()) {
          System.out.println("    - " + error.getErrorCode() + ": " + error.getCount());
        }
      }
      System.out.println("=== End Payload ===\n");

      stub.clientWriteFlagLogs(request);
      logger.debug("Synchronously sent flag log with {} entries", request.getFlagAssignedCount());
    } catch (Exception e) {
      logger.error("Failed to write flag logs synchronously", e);
    }
  }

  /**
   * Shutdown the executor service and wait for pending async writes to complete. This method will
   * block for up to the configured shutdown timeout (default 10 seconds) waiting for pending log
   * writes to complete. Call this when the application is shutting down.
   */
  @Override
  public void shutdown() {
    executorService.shutdown();
    try {
      if (!executorService.awaitTermination(shutdownTimeout.toMillis(), TimeUnit.MILLISECONDS)) {
        logger.warn(
            "Flag logger executor did not terminate within {} seconds, some logs may be lost",
            shutdownTimeout.getSeconds());
        executorService.shutdownNow();
      } else {
        logger.debug("Flag logger executor terminated gracefully");
      }
    } catch (InterruptedException e) {
      logger.warn("Interrupted while waiting for flag logger shutdown", e);
      executorService.shutdownNow();
      Thread.currentThread().interrupt();
    }

    if (channel != null) {
      channel.shutdown();
      try {
        if (!channel.awaitTermination(shutdownTimeout.toMillis(), TimeUnit.MILLISECONDS)) {
          logger.warn(
              "Channel did not terminate within {} seconds, forcing shutdown",
              shutdownTimeout.getSeconds());
          channel.shutdownNow();
        } else {
          logger.debug("Channel terminated gracefully");
        }
      } catch (InterruptedException e) {
        logger.warn("Interrupted while waiting for channel shutdown", e);
        channel.shutdownNow();
        Thread.currentThread().interrupt();
      }
    }
  }

  /**
   * Check if the request has telemetry data with non-empty resolve errors.
   * This ensures error telemetry is sent even when there are no flag assignments.
   */
  private static boolean hasTelemetryWithErrors(WriteFlagLogsRequest request) {
    return request.hasTelemetryData()
        && !request.getTelemetryData().getResolveErrorsList().isEmpty();
  }

  private static InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub
      addAuthInterceptor(
          InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub stub,
          String clientSecret) {
    // Create a stub with authorization header interceptor
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
