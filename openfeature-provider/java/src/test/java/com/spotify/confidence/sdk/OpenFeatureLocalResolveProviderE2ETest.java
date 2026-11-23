package com.spotify.confidence.sdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;

import dev.openfeature.sdk.MutableContext;
import dev.openfeature.sdk.ProviderEvaluation;
import dev.openfeature.sdk.Value;
import java.util.Map;
import java.util.function.Consumer;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.condition.EnabledIfEnvironmentVariable;

/**
 * End-to-end tests against the real module CDN, with the e2e credential, as the Go provider's
 * e2e_test.go; that file is the reference for the flags and variants. They need
 * CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY (set in CI; locally, see
 * openfeature-provider/js/.env.test). Flag logs go to production ingestion.
 */
@EnabledIfEnvironmentVariable(named = "CONFIDENCE_CLIENT_SECRET", matches = ".+")
@EnabledIfEnvironmentVariable(named = "CONFIDENCE_CLIENT_ENCRYPTION_KEY", matches = ".+")
class OpenFeatureLocalResolveProviderE2ETest {

  private static final String INCLUDED_TARGETING_KEY = "user-a";
  private static final String EXCLUDED_TARGETING_KEY = "user-x";

  private static OpenFeatureLocalResolveProvider provider(
      Consumer<LocalProviderConfig.Builder> mod) {
    LocalProviderConfig.Builder cfg = LocalProviderConfig.builder();
    if (mod != null) {
      mod.accept(cfg);
    }
    OpenFeatureLocalResolveProvider provider =
        new OpenFeatureLocalResolveProvider(
            cfg.build(),
            System.getenv("CONFIDENCE_CLIENT_SECRET"),
            System.getenv("CONFIDENCE_CLIENT_ENCRYPTION_KEY"));
    provider.initialize(new MutableContext());
    return provider;
  }

  @Test
  void nonSticky() {
    OpenFeatureLocalResolveProvider p = provider(null);
    try {
      MutableContext ctx = new MutableContext("test-a", Map.of("sticky", new Value(false)));
      ProviderEvaluation<Boolean> bool = p.getBooleanEvaluation("web-sdk-e2e-flag.bool", true, ctx);
      assertEquals(false, bool.getValue());
      assertNull(bool.getErrorCode());
      ProviderEvaluation<Integer> i = p.getIntegerEvaluation("web-sdk-e2e-flag.int", 0, ctx);
      assertNull(i.getErrorCode());
    } finally {
      p.shutdown();
    }
  }

  /**
   * Without a store, the module decides materialized segment membership itself (from the bloom
   * filter compiled into it): both inclusion and exclusion.
   */
  @Test
  void materializedSegmentWithoutStore() {
    OpenFeatureLocalResolveProvider p = provider(null);
    try {
      assertSegmentVariants(p);
    } finally {
      p.shutdown();
    }
  }

  /**
   * The flag has a running experiment with a sticky assignment for test-3 and paused intake. If
   * this breaks, the experiment may have been removed or the materialization cleaned out (see the
   * JS e2e test for how to restore it).
   */
  @Test
  void stickyRemoteStore() {
    OpenFeatureLocalResolveProvider p = provider(c -> c.useRemoteMaterializationStore(true));
    try {
      MutableContext ctx = new MutableContext("test-3", Map.of("sticky", new Value(true)));
      ProviderEvaluation<Double> d = p.getDoubleEvaluation("web-sdk-e2e-flag.double", -1.0, ctx);
      assertEquals(99.99, d.getValue());
      assertEquals("flags/web-sdk-e2e-flag/variants/sticky", d.getVariant());
      assertNull(d.getErrorCode());
    } finally {
      p.shutdown();
    }
  }

  @Test
  void materializedSegmentRemoteStore() {
    OpenFeatureLocalResolveProvider p = provider(c -> c.useRemoteMaterializationStore(true));
    try {
      assertSegmentVariants(p);
    } finally {
      p.shutdown();
    }
  }

  private static void assertSegmentVariants(OpenFeatureLocalResolveProvider p) {
    Map<String, String> units =
        Map.of(
            INCLUDED_TARGETING_KEY, "flags/custom-targeted-flag/variants/cake-exclamation",
            EXCLUDED_TARGETING_KEY, "flags/custom-targeted-flag/variants/default");
    for (Map.Entry<String, String> unit : units.entrySet()) {
      MutableContext ctx = new MutableContext();
      ctx.add("user_id", unit.getKey());
      ProviderEvaluation<String> d =
          p.getStringEvaluation("custom-targeted-flag.message", "client default", ctx);
      assertEquals(unit.getValue(), d.getVariant(), unit.getKey());
    }
  }
}
