package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;

import dev.openfeature.sdk.ImmutableContext;
import java.io.File;
import java.nio.file.Files;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

class OpenFeatureLocalResolveProviderShutdownTest {

  private static final String CLIENT_SECRET = "shutdown-test-secret";
  private static final String ACCOUNT_NAME = "accounts/test-account";

  @BeforeAll
  static void beforeAll() {
    System.setProperty("CONFIDENCE_NUMBER_OF_WASM_INSTANCES", "1");
  }

  @Test
  void shutdownDoesNotWaitForTheNextScheduledPoll() throws Exception {
    final byte[] stateBytes =
        Files.readAllBytes(
            new File(getClass().getResource("/resolver_state_current.pb").getPath()).toPath());

    final OpenFeatureLocalResolveProvider provider =
        new OpenFeatureLocalResolveProvider(
            new TestAccountStateProvider(stateBytes, ACCOUNT_NAME),
            CLIENT_SECRET,
            new UnsupportedMaterializationStore(),
            new NoOpWasmFlagLogger());

    provider.initialize(new ImmutableContext());
    provider.shutdown();

    assertThat(provider.forcedFetcherShutdown)
        .as(
            "shutdown should drop the pending poll and terminate without force-killing the executor")
        .isFalse();
  }
}
