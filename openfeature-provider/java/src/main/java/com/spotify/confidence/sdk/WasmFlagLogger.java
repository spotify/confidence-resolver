package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.admin.v1.LogDestination;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import java.util.List;

interface WasmFlagLogger {
  void write(WriteFlagLogsRequest request);

  void shutdown();

  /**
   * Updates the log routing configuration. First destination is primary, second is fallback.
   *
   * @param destinations the ordered list of log destinations
   * @param accountId the account ID for the Cloudflare ingestor
   */
  default void updateLogRouting(List<LogDestination> destinations, String accountId) {
    // no-op by default for test implementations
  }
}
