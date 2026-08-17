package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;

import com.spotify.confidence.sdk.flags.resolver.v1.SdkId;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import java.util.ArrayList;
import java.util.Map;
import org.junit.jupiter.api.Test;

class WasmLocalResolverTelemetryTest {

  @Test
  void firstFlushIncludesProviderSdk() {
    final var captured = new ArrayList<WriteFlagLogsRequest>();
    final var resolver = new WasmLocalResolver(captured::add, Map.of("encryption", "true"));
    try {
      resolver.flushAllLogs();
    } finally {
      resolver.close();
    }

    assertThat(captured).isNotEmpty();
    final var telemetry = captured.get(0).getTelemetryData();
    assertThat(telemetry.getSdk().getId()).isEqualTo(SdkId.SDK_ID_JAVA_LOCAL_PROVIDER);
    assertThat(telemetry.getSdk().getVersion()).isEqualTo(Version.VERSION);
    assertThat(telemetry.getProviderInitRateCount()).isEqualTo(1);
  }
}
