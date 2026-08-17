package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import com.spotify.confidence.sdk.flags.resolver.v1.Sdk;
import com.spotify.confidence.sdk.flags.resolver.v1.SdkId;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import java.util.ArrayList;
import java.util.Map;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;

class ProviderInitTelemetrySinkTest {
  private static final Sdk SDK =
      Sdk.newBuilder().setId(SdkId.SDK_ID_JAVA_LOCAL_PROVIDER).setVersion("test-version").build();

  @Test
  void emitsOnceAcrossPooledResolvers() {
    final var captured = new ArrayList<WriteFlagLogsRequest>();
    final var sink =
        new ProviderInitTelemetrySink(captured::add, SDK, Map.of("encryption", "true"));

    sink.accept(WriteFlagLogsRequest.getDefaultInstance());
    sink.accept(WriteFlagLogsRequest.getDefaultInstance());
    sink.accept(WriteFlagLogsRequest.getDefaultInstance());

    assertThat(
            captured.stream()
                .mapToInt(request -> request.getTelemetryData().getProviderInitRateCount())
                .sum())
        .isEqualTo(1);
    final var telemetry = captured.get(0).getTelemetryData();
    assertThat(telemetry.getSdk()).isEqualTo(SDK);
    assertThat(telemetry.getProviderInitRate(0).getLabelsMap()).containsEntry("encryption", "true");
  }

  @Test
  void retriesAfterDelegateFailure() {
    final var attempts = new AtomicInteger();
    final var captured = new ArrayList<WriteFlagLogsRequest>();
    final var sink =
        new ProviderInitTelemetrySink(
            request -> {
              if (attempts.getAndIncrement() == 0) {
                throw new IllegalStateException("send failed");
              }
              captured.add(request);
            },
            SDK,
            Map.of());

    assertThatThrownBy(() -> sink.accept(WriteFlagLogsRequest.getDefaultInstance()))
        .isInstanceOf(IllegalStateException.class);
    sink.accept(WriteFlagLogsRequest.getDefaultInstance());

    assertThat(captured).hasSize(1);
    assertThat(captured.get(0).getTelemetryData().getProviderInitRateCount()).isEqualTo(1);
  }

  @Test
  void shutdownBeforePeriodicFlushStillEmitsInit() {
    final var captured = new ArrayList<WriteFlagLogsRequest>();
    final var sink = new ProviderInitTelemetrySink(captured::add, SDK, Map.of());

    sink.emitIfPending();

    assertThat(
            captured.stream()
                .mapToInt(request -> request.getTelemetryData().getProviderInitRateCount())
                .sum())
        .isEqualTo(1);
  }
}
