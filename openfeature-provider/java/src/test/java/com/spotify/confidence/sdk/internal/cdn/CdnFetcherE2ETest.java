package com.spotify.confidence.sdk.internal.cdn;

import static org.assertj.core.api.Assertions.assertThat;

import com.spotify.confidence.sdk.internal.host.Module;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.condition.EnabledIfEnvironmentVariable;

/**
 * Fetches the e2e credential's module from the production CDN and validates it with the host. Needs
 * CONFIDENCE_CLIENT_SECRET and CONFIDENCE_CLIENT_ENCRYPTION_KEY; skipped without them (the
 * Makefile's test target also excludes *E2ETest).
 */
class CdnFetcherE2ETest {

  @Test
  @EnabledIfEnvironmentVariable(named = "CONFIDENCE_CLIENT_SECRET", matches = ".+")
  @EnabledIfEnvironmentVariable(named = "CONFIDENCE_CLIENT_ENCRYPTION_KEY", matches = ".+")
  void fetchProductionModule() {
    byte[] key = CdnFetcher.parseKey(System.getenv("CONFIDENCE_CLIENT_ENCRYPTION_KEY"));
    CdnFetcher fetcher = new CdnFetcher(System.getenv("CONFIDENCE_CLIENT_SECRET"), key, null, null);
    CdnFetcher.FetchedModule module = fetcher.fetch();
    assertThat(module.etag()).isNotEmpty();
    assertThat(module.logToken()).isNotEmpty();
    // The production module must satisfy the host contract.
    assertThat(Module.parse(module.wasm())).isNotNull();
  }
}
