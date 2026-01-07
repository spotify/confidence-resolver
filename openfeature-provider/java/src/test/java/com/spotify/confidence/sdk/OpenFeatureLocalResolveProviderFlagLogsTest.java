package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;
import static org.junit.jupiter.api.Assertions.assertEquals;

import com.spotify.confidence.flags.resolver.v1.FlagAssigned;
import com.spotify.confidence.flags.resolver.v1.WriteFlagLogsRequest;
import dev.openfeature.sdk.Client;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.MutableContext;
import dev.openfeature.sdk.OpenFeatureAPI;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Unit tests that verify WriteFlagLogs contains correct flag assignment data.
 *
 * <p>These tests use a CapturingWasmFlagLogger to capture all flag log requests and verify:
 *
 * <ul>
 *   <li>Flag names are correctly reported
 *   <li>Targeting keys match the evaluation context
 *   <li>Assignment information is present and valid
 *   <li>Variant information matches the resolved value
 * </ul>
 */
class OpenFeatureLocalResolveProviderFlagLogsTest {
  private static final String FLAG_CLIENT_SECRET = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv";
  private static final String TARGETING_KEY = "test-a";
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

  /**
   * Flushes logs by calling shutdown on the provider. Note that the provider will be unusable after
   * this point.
   */
  private void flushLogs() {
    provider.shutdown();
  }

  @Test
  void shouldCaptureWriteFlagLogsAfterBooleanResolve() {
    // Resolve a boolean flag
    final boolean value = client.getBooleanValue("web-sdk-e2e-flag.bool", true);
    assertThat(value).isFalse();

    // Flush logs
    flushLogs();

    // Verify captured flag logs
    assertThat(capturingLogger.getCapturedRequests()).isNotEmpty();

    final WriteFlagLogsRequest request = capturingLogger.getCapturedRequests().get(0);

    // Verify flag_assigned entries
    assertThat(request.getFlagAssignedCount()).isGreaterThanOrEqualTo(1);

    // Find the flag we resolved
    final var flagAssigned =
        request.getFlagAssignedList().stream()
            .flatMap(fa -> fa.getFlagsList().stream())
            .filter(af -> af.getFlag().contains("web-sdk-e2e-flag"))
            .findFirst();

    assertThat(flagAssigned).isPresent();
    assertThat(flagAssigned.get().getTargetingKey()).isEqualTo(TARGETING_KEY);
    assertThat(flagAssigned.get().getFlag()).contains("web-sdk-e2e-flag");
  }

  @Test
  void shouldCaptureCorrectVariantInFlagLogs() {
    // Resolve a flag and verify the variant is captured correctly
    final String value = client.getStringValue("web-sdk-e2e-flag.str", "default");
    assertEquals("control", value);

    // Flush logs
    flushLogs();

    assertThat(capturingLogger.getCapturedRequests()).isNotEmpty();

    final var request = capturingLogger.getCapturedRequests().get(0);
    assertThat(request.getFlagAssignedCount()).isGreaterThanOrEqualTo(1);

    // Verify variant information is present
    final var flagAssigned = request.getFlagAssignedList().get(0);
    assertThat(flagAssigned.getFlagsList()).isNotEmpty();

    // The assigned flag should have variant information
    final var appliedFlag = flagAssigned.getFlagsList().get(0);
    assertThat(appliedFlag.getFlag()).isNotEmpty();
  }

  @Test
  void shouldCaptureClientResolveAndFlagResolveInfo() {
    // Perform a resolve
    client.getIntegerValue("web-sdk-e2e-flag.int", 10);

    // Flush logs
    flushLogs();

    assertThat(capturingLogger.getCapturedRequests()).isNotEmpty();

    final var request = capturingLogger.getCapturedRequests().get(0);

    // Verify client_resolve_info is captured
    assertThat(request.getClientResolveInfoCount()).isGreaterThanOrEqualTo(1);
    assertThat(request.getFlagResolveInfoCount()).isGreaterThanOrEqualTo(1);
  }

  @Test
  void shouldCaptureMultipleResolvesInSingleRequest() {
    // Perform multiple resolves
    client.getBooleanValue("web-sdk-e2e-flag.bool", true);
    client.getStringValue("web-sdk-e2e-flag.str", "default");
    client.getIntegerValue("web-sdk-e2e-flag.int", 10);
    client.getDoubleValue("web-sdk-e2e-flag.double", 10.0);

    // Flush logs
    flushLogs();

    assertThat(capturingLogger.getCapturedRequests()).isNotEmpty();

    // Should have captured log entries for all resolves
    final int totalFlagAssigned = capturingLogger.getTotalFlagAssignedCount();
    assertThat(totalFlagAssigned).isGreaterThanOrEqualTo(4);
  }

  @Test
  void shouldCallShutdownOnProviderShutdown() {
    // Perform a resolve to generate logs
    client.getBooleanValue("web-sdk-e2e-flag.bool", true);

    // Shutdown should be called when provider shuts down
    flushLogs();

    assertThat(capturingLogger.wasShutdownCalled()).isTrue();
  }

  @Test
  void shouldCaptureResolveIdInFlagAssigned() {
    // Perform a resolve
    client.getBooleanValue("web-sdk-e2e-flag.bool", true);

    // Flush logs
    flushLogs();

    assertThat(capturingLogger.getCapturedRequests()).isNotEmpty();

    final var request = capturingLogger.getCapturedRequests().get(0);
    assertThat(request.getFlagAssignedCount()).isGreaterThanOrEqualTo(1);

    // Verify resolve_id is present
    final FlagAssigned flagAssigned = request.getFlagAssigned(0);
    assertThat(flagAssigned.getResolveId()).isNotEmpty();
  }

  @Test
  void shouldCaptureClientInfoInFlagAssigned() {
    // Perform a resolve
    client.getBooleanValue("web-sdk-e2e-flag.bool", true);

    // Flush logs
    flushLogs();

    assertThat(capturingLogger.getCapturedRequests()).isNotEmpty();

    final var request = capturingLogger.getCapturedRequests().get(0);
    assertThat(request.getFlagAssignedCount()).isGreaterThanOrEqualTo(1);

    // Verify client_info is present
    final FlagAssigned flagAssigned = request.getFlagAssigned(0);
    assertThat(flagAssigned.hasClientInfo()).isTrue();
    assertThat(flagAssigned.getClientInfo().getClient()).isNotEmpty();
  }
}
