package com.spotify.confidence.sdk;

import com.spotify.confidence.flags.resolver.v1.WriteFlagLogsRequest;

interface WasmFlagLogger {
  void write(WriteFlagLogsRequest request);

  void writeSync(WriteFlagLogsRequest request);

  void shutdown();
}
