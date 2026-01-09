package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;

/**
 * A no-op implementation of WasmFlagLogger used when flag logging is not needed, typically in test
 * scenarios or when using AccountStateProvider.
 */
class NoOpWasmFlagLogger implements WasmFlagLogger {
  @Override
  public void write(WriteFlagLogsRequest request) {
    // No-op: discard all log requests
  }

  @Override
  public void writeSync(WriteFlagLogsRequest request) {
    // No-op: discard all log requests
  }

  @Override
  public void shutdown() {
    // No-op: nothing to shut down
  }
}
