package com.spotify.confidence.sdk;

import static org.assertj.core.api.AssertionsForInterfaceTypes.assertThat;
import static org.junit.jupiter.api.Assertions.assertEquals;

import dev.openfeature.sdk.Client;
import dev.openfeature.sdk.EvaluationContext;
import dev.openfeature.sdk.FlagEvaluationDetails;
import dev.openfeature.sdk.MutableContext;
import dev.openfeature.sdk.OpenFeatureAPI;
import dev.openfeature.sdk.Value;
import java.util.Map;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * End-to-end tests for OpenFeatureLocalResolveProvider.
 *
 * <p>These tests verify the provider against real Confidence service flags.
 */
class OpenFeatureLocalResolveProviderE2ETest {
  private static final String FLAG_CLIENT_SECRET = "ti5Sipq5EluCYRG7I5cdbpWC3xq7JTWv";
  private static Client client;

  @BeforeAll
  static void setup() {
    final var provider =
        new OpenFeatureLocalResolveProvider(
            LocalProviderConfig.builder().useRemoteMaterializationStore(true).build(),
            FLAG_CLIENT_SECRET);
    final var start = System.currentTimeMillis();
    OpenFeatureAPI.getInstance().setProviderAndWait(provider);
    System.out.println("OpenFeatureAPI started: " + (System.currentTimeMillis() - start));
    // Set evaluation context with targeting key
    final EvaluationContext context = new MutableContext("test-a").add("sticky", false);
    OpenFeatureAPI.getInstance().setEvaluationContext(context);

    client = OpenFeatureAPI.getInstance().getClient();
  }

  @AfterAll
  static void teardown() {
    OpenFeatureAPI.getInstance().getProvider().shutdown();
  }

  @Test
  void shouldResolveBoolean() {
    final boolean value = client.getBooleanValue("web-sdk-e2e-flag.bool", true);
    assertThat(value).isFalse();
  }

  @Test
  void shouldResolveInt() {
    final int value = client.getIntegerValue("web-sdk-e2e-flag.int", 10);
    assertEquals(3, value);
  }

  @Test
  void shouldResolveDouble() {
    final double value = client.getDoubleValue("web-sdk-e2e-flag.double", 10.0);
    assertEquals(3.5, value, 0.001);
  }

  @Test
  void shouldResolveString() {
    final String value = client.getStringValue("web-sdk-e2e-flag.str", "default");
    assertEquals("control", value);
  }

  @Test
  void shouldResolveStruct() {
    final Value value = client.getObjectValue("web-sdk-e2e-flag.obj", new Value());

    assertThat(value.isStructure()).isTrue();
    final Map<String, Value> struct = value.asStructure().asMap();

    assertEquals(4, struct.get("int").asInteger());
    assertEquals("obj control", struct.get("str").asString());
    assertThat(struct.get("bool").asBoolean()).isFalse();
    assertEquals(3.6, struct.get("double").asDouble(), 0.001);
    assertThat(struct.get("obj-obj").asStructure().asMap()).isEmpty();
  }

  @Test
  void shouldResolveSubValueFromStruct() {
    final boolean value = client.getBooleanValue("web-sdk-e2e-flag.obj.bool", true);
    assertThat(value).isFalse();
  }

  @Test
  void shouldResolveSubValueFromStructWithDetails() {
    final FlagEvaluationDetails<Double> details =
        client.getDoubleDetails("web-sdk-e2e-flag.obj.double", 1.0);

    assertEquals(3.6, details.getValue(), 0.001);
    assertEquals("flags/web-sdk-e2e-flag/variants/control", details.getVariant());
    assertEquals("RESOLVE_REASON_MATCH", details.getReason());
  }

  @Test
  void shouldResolveFlagWithStickyResolveAndRemoteMaterializationStore() {
    final EvaluationContext stickyContext = new MutableContext("test-a").add("sticky", true);

    final FlagEvaluationDetails<Double> details =
        client.getDoubleDetails("web-sdk-e2e-flag.double", -1.0, stickyContext);

    // The flag has a running experiment with a sticky assignment. The intake is paused but we
    // should still get the sticky assignment.
    // If this test breaks it could mean that the experiment was removed or that the bigtable
    // materialization was cleaned out.
    assertEquals(99.99, details.getValue(), 0.001);
    assertEquals("flags/web-sdk-e2e-flag/variants/sticky", details.getVariant());
    assertEquals("RESOLVE_REASON_MATCH", details.getReason());
  }

  @Test
  void shouldResolveFlagWithCustomTargetingAndRemoteMaterializationStore() {
    final FlagEvaluationDetails<String> detailsIncluded =
        client.getStringDetails(
            "custom-targeted-flag.message",
            "default",
            new MutableContext().add("user_id", "user-a"));

    // The flag has a rule with a materialized segment targeting. The users `user-a`, `user-b`,
    // `user-c` should get the cake-exclamation variant.
    // If this test breaks it could mean that the rule was removed or that the bigtable
    // materialization was cleaned out.
    assertEquals("Did someone say CAKE?!", detailsIncluded.getValue());
    assertEquals(
        "flags/custom-targeted-flag/variants/cake-exclamation", detailsIncluded.getVariant());
    assertEquals("RESOLVE_REASON_MATCH", detailsIncluded.getReason());

    final FlagEvaluationDetails<String> detailsExcluded =
        client.getStringDetails(
            "custom-targeted-flag.message",
            "default",
            new MutableContext().add("user_id", "user-x"));

    // `user-x` falls through to be assigned to the default variant
    assertEquals("nothing fun", detailsExcluded.getValue());
    assertEquals("flags/custom-targeted-flag/variants/default", detailsExcluded.getVariant());
    assertEquals("RESOLVE_REASON_MATCH", detailsExcluded.getReason());
  }
}
