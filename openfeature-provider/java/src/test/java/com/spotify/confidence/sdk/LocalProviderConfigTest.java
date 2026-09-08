package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;

import org.junit.jupiter.api.Test;

class LocalProviderConfigTest {

  @Test
  void disableExposureCollection_defaultsToFalse() {
    assertThat(new LocalProviderConfig().isDisableExposureCollection()).isFalse();
  }

  @Test
  void disableExposureCollection_canBeEnabled() {
    final LocalProviderConfig config =
        LocalProviderConfig.builder().disableExposureCollection(true).build();
    assertThat(config.isDisableExposureCollection()).isTrue();
  }

  /** Dedup is on by default, so a config that never mentions it must still have it enabled. */
  @Test
  void enableApplyDedup_defaultsToTrue() {
    assertThat(new LocalProviderConfig().isEnableApplyDedup()).isTrue();
    assertThat(LocalProviderConfig.builder().build().isEnableApplyDedup()).isTrue();
  }

  /**
   * Callers written against the previous release pass true explicitly. That must still compile and
   * still leave dedup enabled.
   */
  @Test
  void enableApplyDedup_trueStillEnables() {
    assertThat(LocalProviderConfig.builder().enableApplyDedup(true).build().isEnableApplyDedup())
        .isTrue();
  }

  @Test
  void enableApplyDedup_canBeDisabledViaBuilder() {
    assertThat(LocalProviderConfig.builder().enableApplyDedup(false).build().isEnableApplyDedup())
        .isFalse();
  }

  /**
   * The constructor overload is the opt-out for callers not using the builder. Written in exactly
   * the form the README documents — including {@code DEFAULT_RESOLVER_POOL_SIZE} rather than a bare
   * literal — so that renaming or hiding that constant breaks the build instead of only breaking
   * the documented snippet.
   */
  @Test
  void enableApplyDedup_canBeDisabledViaConstructor() {
    assertThat(
            new LocalProviderConfig(
                    null, null, false, LocalProviderConfig.DEFAULT_RESOLVER_POOL_SIZE, false)
                .isEnableApplyDedup())
        .isFalse();
    assertThat(
            new LocalProviderConfig(
                    null, null, false, LocalProviderConfig.DEFAULT_RESOLVER_POOL_SIZE, true)
                .isEnableApplyDedup())
        .isTrue();
  }
}
