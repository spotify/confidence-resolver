package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;

import ch.qos.logback.classic.Logger;
import ch.qos.logback.classic.spi.ILoggingEvent;
import ch.qos.logback.core.read.ListAppender;
import dev.openfeature.sdk.Client;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.MutableContext;
import dev.openfeature.sdk.OpenFeatureAPI;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.slf4j.LoggerFactory;

/** End-to-end tests that verify WriteFlagLogs successfully sends to the real backend. */
class OpenFeatureLocalResolveProviderFlagLogsE2ETest {
  private static final String FLAG_CLIENT_SECRET = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv";
  private static final String TARGETING_KEY = "test-a";

  @AfterEach
  void teardown() {
    // Clean up OpenFeature state
    try {
      OpenFeatureAPI.getInstance().getProvider().shutdown();
    } catch (Exception ignored) {
    }
  }

  /**
   * Tests that WriteFlagLogs can be successfully sent to the real Confidence backend. This test
   * uses a real provider with real gRPC connection to verify the backend accepts the log data.
   *
   * <p>We use SLF4J's ListAppender to capture log messages and verify no errors occurred.
   */
  @Test
  void shouldSuccessfullySendWriteFlagLogsToRealBackend() throws InterruptedException {
    // Set up logback ListAppender to capture log events
    final Logger logger = (Logger) LoggerFactory.getLogger(GrpcWasmFlagLogger.class);
    final ListAppender<ILoggingEvent> listAppender = new ListAppender<>();
    listAppender.start();
    logger.addAppender(listAppender);

    try {
      final var realProvider = new OpenFeatureLocalResolveProvider(FLAG_CLIENT_SECRET);

      OpenFeatureAPI.getInstance().setProviderAndWait("real-backend-test", realProvider);

      final EvaluationContext context = new MutableContext(TARGETING_KEY).add("sticky", false);
      final Client realClient = OpenFeatureAPI.getInstance().getClient("real-backend-test");

      // Perform a resolve to generate logs
      final boolean value = realClient.getBooleanValue("web-sdk-e2e-flag.bool", true, context);
      assertThat(value).isFalse();

      // Shutdown the provider - this flushes logs to the real backend via gRPC
      realProvider.shutdown();

      // Wait for async operations to complete
      Thread.sleep(1000);

      // Verify no error logs were captured
      final var errorLogs =
          listAppender.list.stream()
              .filter(
                  event -> event.getLevel().levelInt >= ch.qos.logback.classic.Level.ERROR.levelInt)
              .filter(event -> event.getMessage().contains("Failed to write flag logs"))
              .toList();

      assertThat(errorLogs)
          .withFailMessage(
              "Expected no 'Failed to write flag logs' errors, but found: " + errorLogs)
          .isEmpty();

      // The test passes if no error logs were captured, indicating successful backend communication
    } finally {
      // Clean up the appender
      logger.detachAppender(listAppender);
    }
  }
}
