package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;

import org.junit.jupiter.api.Test;

class LocalProviderConfigTest {

  @Test
  void disableExposureCollection_defaultsToFalse() {
    assertThat(new LocalProviderConfig(EncryptionTestSupport.KEY).isDisableExposureCollection())
        .isFalse();
  }

  @Test
  void disableExposureCollection_canBeEnabled() {
    final LocalProviderConfig config =
        LocalProviderConfig.builder()
            .encryptionKey(EncryptionTestSupport.KEY)
            .disableExposureCollection(true)
            .build();
    assertThat(config.isDisableExposureCollection()).isTrue();
  }
}
