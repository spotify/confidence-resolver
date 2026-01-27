package com.spotify.confidence.sdk;

import static org.assertj.core.api.AssertionsForInterfaceTypes.assertThat;
import static org.awaitility.Awaitility.await;
import static org.junit.jupiter.api.Assertions.*;

import com.spotify.confidence.sdk.flags.resolver.v1.InternalFlagLoggerServiceGrpc;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsResponse;
import com.sun.net.httpserver.HttpServer;
import dev.openfeature.sdk.*;
import io.grpc.ClientInterceptor;
import io.grpc.ManagedChannel;
import io.grpc.inprocess.InProcessChannelBuilder;
import io.grpc.inprocess.InProcessServerBuilder;
import io.grpc.stub.StreamObserver;
import io.grpc.testing.GrpcCleanupRule;
import java.io.File;
import java.io.IOException;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.InetSocketAddress;
import java.net.URL;
import java.nio.file.Files;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Tests for OpenFeatureLocalResolveProvider behavior when CDN is unavailable.
 *
 * <p>These tests verify that:
 *
 * <ul>
 *   <li>Initialize doesn't hang indefinitely when CDN is slow/unreachable
 *   <li>Provider starts in NOT_READY state when CDN is unavailable
 *   <li>Provider keeps retrying and transitions to READY when CDN becomes available
 * </ul>
 */
class OpenFeatureLocalResolveProviderCdnFailureTest {
  private static final String FLAG_CLIENT_SECRET = "mkjJruAATQWjeY7foFIWfVAcBWnci2YF";
  private static final String ACCOUNT_NAME = "accounts/test-account";

  @BeforeAll
  static void beforeAll() {
    // Use single WASM instance to speed up tests
    System.setProperty("CONFIDENCE_NUMBER_OF_WASM_INSTANCES", "1");
  }

  private final GrpcCleanupRule grpcCleanup = new GrpcCleanupRule();
  private String serverName;
  private OpenFeatureLocalResolveProvider provider;
  private MockFlagLoggerService mockFlagLoggerService;
  private HttpServer httpServer;
  private AtomicInteger cdnRequestCount = new AtomicInteger();
  private AtomicBoolean cdnShouldHang = new AtomicBoolean(false);
  private AtomicBoolean cdnShouldFail = new AtomicBoolean(false);
  private CountDownLatch cdnHangLatch = new CountDownLatch(1);
  private CountDownLatch cdnRetryLatch = new CountDownLatch(1);

  @BeforeEach
  void setUp() throws Exception {
    serverName = InProcessServerBuilder.generateName();
    cdnRetryLatch = new CountDownLatch(1);

    // Start HTTP server to simulate CDN
    httpServer = HttpServer.create(new InetSocketAddress(0), 0);
    httpServer.createContext(
        "/resolver_state.pb",
        exchange -> {
          cdnRequestCount.incrementAndGet();
          try {
            if (cdnShouldHang.get()) {
              // Simulate hanging CDN - wait for the latch or timeout
              try {
                cdnHangLatch.await(30, TimeUnit.SECONDS);
              } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
              }
              exchange.sendResponseHeaders(503, -1);
              exchange.close();
              return;
            }

            if (cdnShouldFail.get()) {
              // Signal that a retry attempt happened (after the first request)
              if (cdnRequestCount.get() > 1) {
                cdnRetryLatch.countDown();
              }
              // Simulate CDN failure
              exchange.sendResponseHeaders(503, -1);
              exchange.close();
              return;
            }

            // Normal response
            final byte[] rawState =
                Files.readAllBytes(
                    new File(getClass().getResource("/resolver_state_current.pb").getPath())
                        .toPath());

            final var stateRequest =
                com.spotify.confidence.sdk.wasm.Messages.SetResolverStateRequest.newBuilder()
                    .setState(com.google.protobuf.ByteString.copyFrom(rawState))
                    .setAccountId(ACCOUNT_NAME)
                    .build();
            final byte[] responseBytes = stateRequest.toByteArray();

            exchange.getResponseHeaders().set("Content-Type", "application/octet-stream");
            exchange.getResponseHeaders().set("ETag", "\"test-etag\"");
            exchange.sendResponseHeaders(200, responseBytes.length);
            try (OutputStream os = exchange.getResponseBody()) {
              os.write(responseBytes);
            }
          } finally {
            exchange.close();
          }
        });
    httpServer.start();

    mockFlagLoggerService = new MockFlagLoggerService();

    grpcCleanup.register(
        InProcessServerBuilder.forName(serverName)
            .directExecutor()
            .addService(mockFlagLoggerService)
            .build()
            .start());
  }

  private OpenFeatureLocalResolveProvider createProvider() {
    final ChannelFactory testChannelFactory =
        new ChannelFactory() {
          @Override
          public ManagedChannel create(String target, List<ClientInterceptor> interceptors) {
            InProcessChannelBuilder builder = InProcessChannelBuilder.forName(serverName);
            if (!interceptors.isEmpty()) {
              builder.intercept(interceptors.toArray(new ClientInterceptor[0]));
            }
            return builder.build();
          }
        };

    final HttpClientFactory testHttpClientFactory =
        new HttpClientFactory() {
          @Override
          public HttpURLConnection create(String url) throws IOException {
            final String localUrl =
                "http://localhost:" + httpServer.getAddress().getPort() + "/resolver_state.pb";
            HttpURLConnection conn = (HttpURLConnection) new URL(localUrl).openConnection();
            // Set short timeouts to make tests run faster
            conn.setConnectTimeout(1000);
            conn.setReadTimeout(1000);
            return conn;
          }

          @Override
          public void shutdown() {}
        };

    final LocalProviderConfig config =
        new LocalProviderConfig(testChannelFactory, testHttpClientFactory);
    return new OpenFeatureLocalResolveProvider(config, FLAG_CLIENT_SECRET);
  }

  @AfterEach
  void tearDown() {
    cdnHangLatch.countDown(); // Release any hanging requests
    if (provider != null) {
      provider.shutdown();
    }
    if (httpServer != null) {
      httpServer.stop(0);
    }
  }

  @Test
  void testInitializeWithCdnDown_shouldNotBlock() throws Exception {
    cdnShouldFail.set(true);
    provider = createProvider();

    assertEquals(ProviderState.NOT_READY, provider.getState());

    // Initialize should complete without blocking, even if CDN is down
    long startTime = System.currentTimeMillis();
    provider.initialize(new ImmutableContext());
    long duration = System.currentTimeMillis() - startTime;

    // Should complete quickly (under 5 seconds), not hang
    assertThat(duration).isLessThan(5000);

    // Provider should remain in NOT_READY state when CDN is down
    assertEquals(ProviderState.NOT_READY, provider.getState());

    // Should have attempted to fetch from CDN
    assertThat(cdnRequestCount.get()).isGreaterThanOrEqualTo(1);
  }

  @Test
  void testInitializeWithHangingCdn_shouldTimeout() throws Exception {
    cdnShouldHang.set(true);
    provider = createProvider();

    assertEquals(ProviderState.NOT_READY, provider.getState());

    // Initialize should timeout and not block indefinitely
    long startTime = System.currentTimeMillis();
    provider.initialize(new ImmutableContext());
    long duration = System.currentTimeMillis() - startTime;

    // Should timeout within a reasonable time (5 seconds max)
    assertThat(duration).isLessThan(5000);

    // Provider should remain in NOT_READY state
    assertEquals(ProviderState.NOT_READY, provider.getState());
  }

  @Test
  void testProviderRecoversWhenCdnBecomesAvailable() throws Exception {
    cdnShouldFail.set(true);
    provider = createProvider();

    // Initialize with CDN down
    provider.initialize(new ImmutableContext());
    assertEquals(ProviderState.NOT_READY, provider.getState());

    // Wait for at least one retry attempt to ensure the retry loop is running
    // This prevents a race condition where we enable CDN before the retry mechanism kicks in
    assertTrue(
        cdnRetryLatch.await(5, TimeUnit.SECONDS),
        "Provider should have attempted at least one retry");

    // Now CDN becomes available
    cdnShouldFail.set(false);

    // Wait for the background retry to succeed and WASM to initialize
    // Using single WASM instance (via system property) to speed up tests
    await()
        .atMost(20, TimeUnit.SECONDS)
        .pollInterval(500, TimeUnit.MILLISECONDS)
        .untilAsserted(() -> assertEquals(ProviderState.READY, provider.getState()));

    assertThat(cdnRequestCount.get()).isGreaterThan(1); // Should have retried
  }

  @Test
  void testProviderKeepsRetryingWhenCdnIsDown() {
    cdnShouldFail.set(true);
    provider = createProvider();

    provider.initialize(new ImmutableContext());
    int initialRequestCount = cdnRequestCount.get();

    // Wait until retries have happened
    await()
        .atMost(5, TimeUnit.SECONDS)
        .untilAsserted(
            () ->
                assertThat(cdnRequestCount.get())
                    .withFailMessage("Provider should keep retrying to fetch state from CDN")
                    .isGreaterThan(initialRequestCount));

    assertEquals(
        ProviderState.NOT_READY,
        provider.getState(),
        "Provider should remain NOT_READY while CDN is down");
  }

  @Test
  void testFlagEvaluationWhileNotReady_shouldThrowOrReturnDefault() throws Exception {
    cdnShouldFail.set(true);
    provider = createProvider();

    provider.initialize(new ImmutableContext());
    assertEquals(ProviderState.NOT_READY, provider.getState());

    final ImmutableContext context =
        new ImmutableContext("test_user", java.util.Map.of("user_id", new Value("test_user")));

    // Attempting to evaluate a flag while NOT_READY should throw an exception
    // because the WASM resolver hasn't been initialized with state
    assertThrows(
        Exception.class,
        () -> provider.getBooleanEvaluation("some-flag", false, context),
        "Flag evaluation should fail when provider is NOT_READY");
  }

  @Test
  void testFlagEvaluationSucceedsAfterRecovery() throws Exception {
    cdnShouldFail.set(true);
    provider = createProvider();

    // Initialize with CDN down
    provider.initialize(new ImmutableContext());
    assertEquals(ProviderState.NOT_READY, provider.getState());

    // Wait for at least one retry attempt to ensure the retry loop is running
    assertTrue(
        cdnRetryLatch.await(5, TimeUnit.SECONDS),
        "Provider should have attempted at least one retry");

    // CDN becomes available
    cdnShouldFail.set(false);

    // Wait for recovery and WASM initialization
    // Using single WASM instance (via system property) to speed up tests
    await()
        .atMost(20, TimeUnit.SECONDS)
        .pollInterval(500, TimeUnit.MILLISECONDS)
        .untilAsserted(() -> assertEquals(ProviderState.READY, provider.getState()));

    // Now flag evaluation should work
    final ImmutableContext context =
        new ImmutableContext(
            "tutorial_visitor", java.util.Map.of("visitor_id", new Value("tutorial_visitor")));

    // This should not throw - provider is now READY
    ProviderEvaluation<String> evaluation =
        provider.getStringEvaluation("tutorial-feature.message", "default", context);

    assertThat(evaluation.getValue()).isNotEqualTo("default");
    assertThat(evaluation.getValue()).contains("Confidence");
  }

  private static class MockFlagLoggerService
      extends InternalFlagLoggerServiceGrpc.InternalFlagLoggerServiceImplBase {
    private final AtomicInteger requestCount = new AtomicInteger(0);

    @Override
    public void clientWriteFlagLogs(
        WriteFlagLogsRequest request, StreamObserver<WriteFlagLogsResponse> responseObserver) {
      requestCount.incrementAndGet();
      responseObserver.onNext(WriteFlagLogsResponse.getDefaultInstance());
      responseObserver.onCompleted();
    }

    public int getRequestCount() {
      return requestCount.get();
    }
  }
}
