package com.spotify.confidence.sdk;

import static org.assertj.core.api.AssertionsForInterfaceTypes.assertThat;

import dev.openfeature.sdk.*;
import org.junit.jupiter.api.*;

class OpenFeatureLocalResolveProviderEncryptedIT {
  private static final String FLAG_CLIENT_SECRET = System.getenv("CONFIDENCE_CLIENT_SECRET");
  private static final String ENCRYPTION_KEY = System.getenv("CONFIDENCE_CLIENT_ENCRYPTION_KEY");
  private static Client client;

  @BeforeAll
  static void setup() {
    final var provider =
        new OpenFeatureLocalResolveProvider(
            LocalProviderConfig.builder().encryptionKey(ENCRYPTION_KEY).build(),
            FLAG_CLIENT_SECRET);
    OpenFeatureAPI.getInstance().setProviderAndWait("encrypted-e2e", provider);
    final EvaluationContext context = new MutableContext("test-a").add("sticky", false);
    OpenFeatureAPI.getInstance().setEvaluationContext(context);
    client = OpenFeatureAPI.getInstance().getClient("encrypted-e2e");
  }

  @AfterAll
  static void teardown() {
    OpenFeatureAPI.getInstance().shutdown();
  }

  @Test
  void shouldResolveBooleanViaEncryptedState() {
    assertThat(client.getBooleanValue("web-sdk-e2e-flag.bool", true)).isFalse();
  }

  @Test
  void shouldResolveStringViaEncryptedState() {
    assertThat(client.getStringValue("web-sdk-e2e-flag.str", "default")).isEqualTo("control");
  }

  @Test
  void shouldResolveDetailsViaEncryptedState() {
    final FlagEvaluationDetails<Double> details =
        client.getDoubleDetails("web-sdk-e2e-flag.obj.double", 1.0);
    assertThat(details.getValue()).isEqualTo(3.6);
    assertThat(details.getReason()).isEqualTo("MATCH");
  }
}
