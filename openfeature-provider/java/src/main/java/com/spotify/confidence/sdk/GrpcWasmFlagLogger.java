package com.spotify.confidence.sdk;

import static com.spotify.confidence.sdk.GrpcUtil.createConfidenceChannel;

import com.google.common.annotations.VisibleForTesting;
import com.spotify.confidence.sdk.flags.admin.v1.LogDestination;
import com.spotify.confidence.sdk.flags.resolver.v1.IngestFlagLogsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.InternalFlagLoggerServiceGrpc;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import io.grpc.*;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.time.Duration;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

@FunctionalInterface
interface FlagLogWriter {
  void write(WriteFlagLogsRequest request);
}

public class GrpcWasmFlagLogger implements WasmFlagLogger {
  private static final Logger logger = LoggerFactory.getLogger(GrpcWasmFlagLogger.class);
  private static final Duration DEFAULT_SHUTDOWN_TIMEOUT = Duration.ofSeconds(10);
  private static final int STATS_WINDOW = 10;
  private static final String CLOUDFLARE_INGEST_URL =
      "https://epx-flags-logs.experimentation-platform.workers.dev/v1/flagLogs:ingest";
  private final InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceBlockingStub stub;
  private final ExecutorService executorService;
  private final FlagLogWriter writer;
  private final Duration shutdownTimeout;
  private final AtomicLong attempts = new AtomicLong();
  private final AtomicLong failures = new AtomicLong();
  private final AtomicLong successes = new AtomicLong();
  private final String clientSecret;
  private final HttpClientFactory httpClientFactory;
  private final AtomicReference<List<LogDestination>> logDestinations =
      new AtomicReference<>(Collections.emptyList());
  private final AtomicReference<String> accountId = new AtomicReference<>("");
  private ManagedChannel channel;

  @VisibleForTesting
  public GrpcWasmFlagLogger(String clientSecret, FlagLogWriter writer) {
    this.clientSecret = clientSecret;
    this.httpClientFactory = new DefaultHttpClientFactory();
    this.stub = createAuthStub(new DefaultChannelFactory(), clientSecret);
    this.executorService = Executors.newCachedThreadPool();
    this.writer = writer;
    this.shutdownTimeout = DEFAULT_SHUTDOWN_TIMEOUT;
  }

  @VisibleForTesting
  public GrpcWasmFlagLogger(String clientSecret, FlagLogWriter writer, Duration shutdownTimeout) {
    this.clientSecret = clientSecret;
    this.httpClientFactory = new DefaultHttpClientFactory();
    this.stub = createAuthStub(new DefaultChannelFactory(), clientSecret);
    this.executorService = Executors.newCachedThreadPool();
    this.writer = writer;
    this.shutdownTimeout = shutdownTimeout;
  }

  public GrpcWasmFlagLogger(
      String clientSecret, ChannelFactory channelFactory, HttpClientFactory httpClientFactory) {
    this.clientSecret = clientSecret;
    this.httpClientFactory = httpClientFactory;
    this.stub = createAuthStub(channelFactory, clientSecret);
    this.executorService = Executors.newCachedThreadPool();
    this.shutdownTimeout = DEFAULT_SHUTDOWN_TIMEOUT;
    this.writer =
        request ->
            executorService.submit(
                () -> {
                  sendWithFailover(request);
                  if (attempts.incrementAndGet() % STATS_WINDOW == 0) {
                    long failCount = failures.getAndSet(0);
                    if (failCount > 0) {
                      logger.warn("Flag log write failures: {}/{}", failCount, STATS_WINDOW);
                    }
                  }
                });
  }

  /** Kept for backward compatibility. Uses default HTTP client factory. */
  public GrpcWasmFlagLogger(String clientSecret, ChannelFactory channelFactory) {
    this(clientSecret, channelFactory, new DefaultHttpClientFactory());
  }

  @Override
  public void updateLogRouting(List<LogDestination> destinations, String accountId) {
    this.logDestinations.set(destinations);
    this.accountId.set(accountId);
    logger.debug("Updated log routing: destinations={}, accountId={}", destinations, accountId);
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
        && !request.hasTelemetryData()) {
      logger.debug("Skipping empty flag log request");
      return;
    }

    writer.write(request);
  }

  /**
   * Sends the log request using the configured destinations with failover. The first destination is
   * primary; the second (if present) is used as fallback on error. If no destinations are
   * configured, defaults to the gRPC Edge path.
   */
  private void sendWithFailover(WriteFlagLogsRequest request) {
    final List<LogDestination> destinations = logDestinations.get();

    // Default to Edge when no destinations configured
    if (destinations.isEmpty()) {
      try {
        sendToEdge(request);
        successes.incrementAndGet();
      } catch (Exception e) {
        failures.incrementAndGet();
      }
      return;
    }

    final LogDestination primary = destinations.get(0);
    try {
      sendToDestination(primary, request);
      successes.incrementAndGet();
      logger.debug(
          "Successfully sent flag log via {} with {} assigned, {} client_resolve_info, {} flag_resolve_info",
          primary,
          request.getFlagAssignedCount(),
          request.getClientResolveInfoCount(),
          request.getFlagResolveInfoCount());
    } catch (Exception primaryEx) {
      if (destinations.size() > 1) {
        final LogDestination fallback = destinations.get(1);
        logger.warn("Primary destination {} failed, trying fallback {}", primary, fallback);
        try {
          sendToDestination(fallback, request);
          successes.incrementAndGet();
          logger.debug(
              "Successfully sent flag log via fallback {} with {} assigned, {} client_resolve_info, {} flag_resolve_info",
              fallback,
              request.getFlagAssignedCount(),
              request.getClientResolveInfoCount(),
              request.getFlagResolveInfoCount());
        } catch (Exception fallbackEx) {
          failures.incrementAndGet();
          logger.warn("Fallback destination {} also failed", fallback, fallbackEx);
        }
      } else {
        failures.incrementAndGet();
      }
    }
  }

  private void sendToDestination(LogDestination destination, WriteFlagLogsRequest request) {
    switch (destination) {
      case LOG_DESTINATION_CLOUDFLARE:
        sendToCloudflare(request);
        break;
      case LOG_DESTINATION_SPOTIFY_EDGE:
      case LOG_DESTINATION_UNSPECIFIED:
      default:
        sendToEdge(request);
        break;
    }
  }

  private void sendToEdge(WriteFlagLogsRequest request) {
    stub.clientWriteFlagLogs(request);
  }

  private void sendToCloudflare(WriteFlagLogsRequest request) {
    final IngestFlagLogsRequest ingestRequest =
        IngestFlagLogsRequest.newBuilder().setAccountId(accountId.get()).setBatch(request).build();
    final byte[] body = ingestRequest.toByteArray();

    try {
      final HttpURLConnection conn = httpClientFactory.create(CLOUDFLARE_INGEST_URL);
      conn.setRequestMethod("POST");
      conn.setDoOutput(true);
      conn.setRequestProperty("Content-Type", "application/protobuf");
      conn.setRequestProperty("Authorization", "ClientSecret " + clientSecret);
      conn.setRequestProperty("Content-Length", String.valueOf(body.length));

      try (OutputStream os = conn.getOutputStream()) {
        os.write(body);
      }

      final int responseCode = conn.getResponseCode();
      if (responseCode < 200 || responseCode >= 300) {
        throw new RuntimeException("Cloudflare ingest returned HTTP " + responseCode);
      }
    } catch (RuntimeException e) {
      throw e;
    } catch (Exception e) {
      throw new RuntimeException("Failed to send flag logs to Cloudflare", e);
    }
  }

  @Override
  public long[] drainFlushCounters() {
    return new long[] {successes.getAndSet(0), failures.getAndSet(0)};
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

    httpClientFactory.shutdown();
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
