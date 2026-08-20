package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;

import org.junit.jupiter.api.Test;

class LocalProviderConfigTest {

  @Test
  void skipApply_defaultsToFalse() {
    assertThat(new LocalProviderConfig().isSkipApply()).isFalse();
  }

  @Test
  void skipApply_canBeEnabled() {
    final LocalProviderConfig config = LocalProviderConfig.builder().skipApply(true).build();
    assertThat(config.isSkipApply()).isTrue();
  }
}
