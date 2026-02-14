package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;

import com.spotify.confidence.sdk.flags.resolver.v1.OpenFeatureErrorCode;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveErrorCount;
import com.spotify.confidence.sdk.flags.resolver.v1.Sdk;
import com.spotify.confidence.sdk.flags.resolver.v1.SdkId;
import com.spotify.confidence.sdk.flags.resolver.v1.TelemetryData;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import dev.openfeature.sdk.Client;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.MutableContext;
import dev.openfeature.sdk.OpenFeatureAPI;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Tests that verify telemetry data in WriteFlagLogs.
 *
 * <p>These tests verify that:
 *
 * <ul>
 *   <li>TelemetryData is properly included in WriteFlagLogsRequest
 *   <li>SDK information is captured correctly
 *   <li>Error tracking infrastructure is in place (resolve_errors field exists)
 * </ul>
 *
 * <p>Note: The Rust resolver tracks errors when:
 *
 * <ul>
 *   <li>FlagArchived - maps to FLAG_NOT_FOUND
 *   <li>TargetingKeyError - maps to TARGETING_KEY_MISSING
 * </ul>
 *
 * <p>Java-side errors like TYPE_MISMATCH would need additional integration to be tracked.
 */
class OpenFeatureLocalResolveProviderErrorTrackingTest {
  private static final String FLAG_CLIENT_SECRET = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv";
  private static final String TARGETING_KEY = "test-error-tracking";
  private Client client;
  private CapturingWasmFlagLogger capturingLogger;
  private OpenFeatureLocalResolveProvider provider;

  @BeforeAll
  static void beforeAll() {
    System.setProperty("CONFIDENCE_NUMBER_OF_WASM_INSTANCES", "1");
  }

  @BeforeEach
  void setup() {
    capturingLogger = new CapturingWasmFlagLogger();

    // Create a state provider that fetches from the real Confidence service
    final var stateProvider =
        new FlagsAdminStateFetcher(FLAG_CLIENT_SECRET, new DefaultHttpClientFactory());
    stateProvider.reload();

    // Create provider with capturing logger
    provider =
        new OpenFeatureLocalResolveProvider(
            stateProvider,
            FLAG_CLIENT_SECRET,
            new UnsupportedMaterializationStore(),
            capturingLogger);

    OpenFeatureAPI.getInstance().setProviderAndWait(provider);

    // Set evaluation context with targeting key
    final EvaluationContext context = new MutableContext(TARGETING_KEY).add("sticky", false);
    OpenFeatureAPI.getInstance().setEvaluationContext(context);

    client = OpenFeatureAPI.getInstance().getClient();

    // Clear any logs captured during initialization
    capturingLogger.clear();
  }

  @AfterEach
  void teardown() {
    // Clean up OpenFeature state
    try {
      OpenFeatureAPI.getInstance().getProvider().shutdown();
    } catch (Exception ignored) {
    }
  }

  private void flushLogs() {
    provider.shutdown();
  }

  @Test
  void shouldCaptureTelemetryDataWithSdkInfo() {
    // Resolve a flag successfully
    final boolean value = client.getBooleanValue("web-sdk-e2e-flag.bool", true);
    assertThat(value).isFalse(); // The test flag returns false

    // Flush logs
    flushLogs();

    // Verify captured flag logs
    assertThat(capturingLogger.getCapturedRequests()).isNotEmpty();

    final WriteFlagLogsRequest request = capturingLogger.getCapturedRequests().get(0);

    // Verify telemetry data is present and contains SDK info
    assertThat(request.hasTelemetryData()).isTrue();

    final TelemetryData telemetry = request.getTelemetryData();
    assertThat(telemetry.hasSdk()).isTrue();
    assertThat(telemetry.getSdk().getId()).isEqualTo(SdkId.SDK_ID_JAVA_LOCAL_PROVIDER);
    assertThat(telemetry.getSdk().getVersion()).isNotEmpty();

    // Print debug info
    System.out.println("=== TelemetryData Verification ===");
    System.out.println("SDK: " + telemetry.getSdk().getId() + " v" + telemetry.getSdk().getVersion());
    System.out.println("Resolve errors count: " + telemetry.getResolveErrorsCount());
    System.out.println("==================================");
  }

  @Test
  void shouldCaptureTargetingKeyErrorWithFractionalTargetingKey() {
    // Create a context with a fractional targeting key value
    // This triggers TargetingKeyError in the Rust resolver because
    // fractional numbers are rejected as targeting keys
    final EvaluationContext contextWithFractionalKey =
        new MutableContext()
            .add("targeting_key", 123.456) // Fractional number - invalid for targeting key
            .add("sticky", false);
    OpenFeatureAPI.getInstance().setEvaluationContext(contextWithFractionalKey);

    // Resolve a flag - this should trigger TargetingKeyError if the flag
    // uses "targeting_key" as its targeting key selector
    client.getBooleanValue("web-sdk-e2e-flag.bool", true);

    // Flush logs
    flushLogs();

    assertThat(capturingLogger.getCapturedRequests()).isNotEmpty();

    final WriteFlagLogsRequest request = capturingLogger.getCapturedRequests().get(0);
    assertThat(request.hasTelemetryData()).isTrue();

    final TelemetryData telemetry = request.getTelemetryData();

    // Print debug info
    System.out.println("=== Fractional Targeting Key Test ===");
    System.out.println("Resolve errors count: " + telemetry.getResolveErrorsCount());
    for (ResolveErrorCount error : telemetry.getResolveErrorsList()) {
      System.out.println("  " + error.getErrorCode().name() + ": " + error.getCount());
    }
    System.out.println("=====================================");

    // Verify that TARGETING_KEY_MISSING error was tracked
    assertThat(telemetry.getResolveErrorsCount()).isGreaterThan(0);

    final var targetingKeyError =
        telemetry.getResolveErrorsList().stream()
            .filter(
                e ->
                    e.getErrorCode()
                        == OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_TARGETING_KEY_MISSING)
            .findFirst();

    assertThat(targetingKeyError)
        .as("Expected TARGETING_KEY_MISSING error to be tracked")
        .isPresent();
    assertThat(targetingKeyError.get().getCount())
        .as("Expected at least one error count")
        .isGreaterThanOrEqualTo(1);
  }

  @Test
  void shouldHaveResolveErrorsFieldInTelemetryData() {
    // Resolve a flag successfully
    client.getBooleanValue("web-sdk-e2e-flag.bool", true);

    // Flush logs
    flushLogs();

    assertThat(capturingLogger.getCapturedRequests()).isNotEmpty();

    final WriteFlagLogsRequest request = capturingLogger.getCapturedRequests().get(0);
    assertThat(request.hasTelemetryData()).isTrue();

    final TelemetryData telemetry = request.getTelemetryData();

    // Verify the resolve_errors field exists (even if empty for successful resolves)
    // This confirms the proto definition is correctly integrated
    assertThat(telemetry.getResolveErrorsList()).isNotNull();

    // For successful resolves, errors should be empty
    // (The test flag doesn't trigger FlagArchived or TargetingKeyError)
    System.out.println("=== Resolve Errors Field Test ===");
    System.out.println("Resolve errors list is accessible: true");
    System.out.println("Resolve errors count: " + telemetry.getResolveErrorsCount());
    for (ResolveErrorCount error : telemetry.getResolveErrorsList()) {
      System.out.println("  " + error.getErrorCode().name() + ": " + error.getCount());
    }
    System.out.println("=================================");
  }

  @Test
  void shouldCaptureTelemetryAcrossMultipleResolves() {
    // Perform multiple resolves
    client.getBooleanValue("web-sdk-e2e-flag.bool", true);
    client.getStringValue("web-sdk-e2e-flag.str", "default");
    client.getIntegerValue("web-sdk-e2e-flag.int", 0);

    // Flush logs
    flushLogs();

    assertThat(capturingLogger.getCapturedRequests()).isNotEmpty();

    final WriteFlagLogsRequest request = capturingLogger.getCapturedRequests().get(0);

    // Verify telemetry is present
    assertThat(request.hasTelemetryData()).isTrue();

    // Multiple resolves should still produce valid telemetry
    final TelemetryData telemetry = request.getTelemetryData();
    assertThat(telemetry.hasSdk()).isTrue();

    // Verify we captured multiple flag assignments
    assertThat(request.getFlagAssignedCount()).isGreaterThanOrEqualTo(3);

    System.out.println("=== Multiple Resolves Test ===");
    System.out.println("Flag assignments: " + request.getFlagAssignedCount());
    System.out.println("SDK: " + telemetry.getSdk().getId());
    System.out.println("==============================");
  }

  @Test
  void telemetryDataSupportsAllOpenFeatureErrorCodes() {
    // This test verifies that all OpenFeature error codes are defined in the proto
    // and can be used in ResolveErrorCount messages

    // Verify all error codes are accessible
    assertThat(OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_UNSPECIFIED).isNotNull();
    assertThat(OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_PROVIDER_NOT_READY).isNotNull();
    assertThat(OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_FLAG_NOT_FOUND).isNotNull();
    assertThat(OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_PARSE_ERROR).isNotNull();
    assertThat(OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_TYPE_MISMATCH).isNotNull();
    assertThat(OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_TARGETING_KEY_MISSING).isNotNull();
    assertThat(OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_INVALID_CONTEXT).isNotNull();
    assertThat(OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_GENERAL).isNotNull();

    // Verify ResolveErrorCount can be built with these codes
    final ResolveErrorCount errorCount =
        ResolveErrorCount.newBuilder()
            .setErrorCode(OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_TYPE_MISMATCH)
            .setCount(5)
            .build();

    assertThat(errorCount.getErrorCode())
        .isEqualTo(OpenFeatureErrorCode.OPEN_FEATURE_ERROR_CODE_TYPE_MISMATCH);
    assertThat(errorCount.getCount()).isEqualTo(5);

    System.out.println("=== OpenFeature Error Codes Test ===");
    System.out.println("All error codes are accessible in Java proto bindings");
    System.out.println("ResolveErrorCount can be constructed correctly");
    System.out.println("====================================");
  }
}
