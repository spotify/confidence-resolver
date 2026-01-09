package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;

interface WasmFlagLogger {
  void write(WriteFlagLogsRequest request);

  void writeSync(WriteFlagLogsRequest request);

  void shutdown();
}
