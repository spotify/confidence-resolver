package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

import com.spotify.confidence.sdk.internal.engine.ExposureMode;
import java.time.Duration;
import org.junit.jupiter.api.Test;

class LocalProviderConfigTest {

  @Test
  void defaults() {
    LocalProviderConfig config = LocalProviderConfig.builder().build();

    assertNull(config.getMaterializationStore());
    assertFalse(config.isUseRemoteMaterializationStore());
    assertEquals(0, config.getInstances(), "zero means the engine default");
    assertEquals(0, config.getMaxMemoryBytes(), "zero means the engine default");
    assertNull(config.getStatePollInterval());
    assertNull(config.getRetryInterval());
    assertEquals(
        LocalProviderConfig.DEFAULT_INITIALIZATION_TIMEOUT, config.getInitializationTimeout());
    assertEquals(ExposureMode.DEDUPLICATED, config.getExposureMode());
    assertFalse(config.isInterpreter());
    assertNotNull(config.getHttpClientFactory());
    assertNotNull(config.getLogger());
  }

  @Test
  void exposureModeFallsBackToDeduplicated() {
    LocalProviderConfig config =
        LocalProviderConfig.builder().exposureMode(ExposureMode.ALL).exposureMode(null).build();
    assertEquals(ExposureMode.DEDUPLICATED, config.getExposureMode());
  }

  @Test
  void initializationTimeoutFallsBackToDefault() {
    LocalProviderConfig config =
        LocalProviderConfig.builder()
            .initializationTimeout(Duration.ZERO)
            .initializationTimeout(null)
            .build();
    assertEquals(
        LocalProviderConfig.DEFAULT_INITIALIZATION_TIMEOUT, config.getInitializationTimeout());
  }

  @Test
  void storeAndRemoteStoreAreMutuallyExclusive() {
    assertThrows(
        IllegalArgumentException.class,
        () ->
            LocalProviderConfig.builder()
                .materializationStore(new InMemoryMaterializationStoreExample())
                .useRemoteMaterializationStore(true)
                .build());
  }
}
