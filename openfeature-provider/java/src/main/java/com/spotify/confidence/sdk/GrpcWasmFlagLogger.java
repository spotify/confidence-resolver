package com.spotify.confidence.sdk;

import static com.spotify.confidence.sdk.GrpcUtil.createConfidenceChannel;

import com.google.common.annotations.VisibleForTesting;
import com.spotify.confidence.sdk.flags.resolver.v1.InternalFlagLoggerServiceGrpc;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import io.grpc.*;
import java.time.Duration;
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
        && request.getFlagResolveInfoList().isEmpty()) {
      logger.debug("Skipping empty flag log request");
      return;
    }

    writer.write(request);
  }

  @Override
  public void writeSync(WriteFlagLogsRequest request) {
    if (request.getClientResolveInfoList().isEmpty()
        && request.getFlagAssignedList().isEmpty()
        && request.getFlagResolveInfoList().isEmpty()) {
      logger.debug("Skipping empty flag log request");
      return;
    }

    try {
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
