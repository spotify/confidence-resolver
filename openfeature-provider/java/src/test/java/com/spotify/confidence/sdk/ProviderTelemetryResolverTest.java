package com.spotify.confidence.sdk;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.RegisterResolveRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.Sdk;
import com.spotify.confidence.sdk.flags.resolver.v1.SdkId;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import java.util.ArrayList;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Consumer;
import org.junit.jupiter.api.Test;

class ProviderTelemetryResolverTest {
  private static final Sdk SDK =
      Sdk.newBuilder().setId(SdkId.SDK_ID_JAVA_LOCAL_PROVIDER).setVersion("test-version").build();

  @Test
  void emitsOnceAcrossPooledFlushes() {
    final var captured = new ArrayList<WriteFlagLogsRequest>();
    final var resolver =
        new ProviderTelemetryResolver(
            captured::add,
            SDK,
            Map.of("encryption", "true"),
            sink -> new TelemetryTestResolver(sink, 3));

    resolver.flushAllLogs();

    assertThat(providerInitEventCount(captured)).isEqualTo(1);
    final var telemetry = captured.get(0).getTelemetryData();
    assertThat(telemetry.getSdk()).isEqualTo(SDK);
    assertThat(telemetry.getProviderInitRate(0).getLabelsMap()).containsEntry("encryption", "true");
  }

  @Test
  void closeEmitsWithoutResolve() {
    final var captured = new ArrayList<WriteFlagLogsRequest>();
    final var resolver =
        new ProviderTelemetryResolver(
            captured::add, SDK, Map.of(), sink -> new TelemetryTestResolver(sink, 0));

    resolver.close();

    assertThat(providerInitEventCount(captured)).isEqualTo(1);
  }

  @Test
  void retriesAfterSinkFailure() {
    final var attempts = new AtomicInteger();
    final var captured = new ArrayList<WriteFlagLogsRequest>();
    final var resolver =
        new ProviderTelemetryResolver(
            request -> {
              if (attempts.getAndIncrement() == 0) {
                throw new IllegalStateException("send failed");
              }
              captured.add(request);
            },
            SDK,
            Map.of(),
            sink -> new TelemetryTestResolver(sink, 1));

    assertThatThrownBy(resolver::flushAllLogs).isInstanceOf(IllegalStateException.class);
    resolver.flushAllLogs();

    assertThat(providerInitEventCount(captured)).isEqualTo(1);
  }

  private static int providerInitEventCount(ArrayList<WriteFlagLogsRequest> requests) {
    return requests.stream()
        .mapToInt(request -> request.getTelemetryData().getProviderInitRateCount())
        .sum();
  }

  private static final class TelemetryTestResolver implements LocalResolver {
    private final Consumer<WriteFlagLogsRequest> sink;
    private final int flushCount;

    private TelemetryTestResolver(Consumer<WriteFlagLogsRequest> sink, int flushCount) {
      this.sink = sink;
      this.flushCount = flushCount;
    }

    @Override
    public void setResolverState(byte[] state, String accountId, Sdk sdk) {}

    @Override
    public CompletionStage<ResolveProcessResponse> resolveProcess(ResolveProcessRequest request) {
      return CompletableFuture.completedFuture(ResolveProcessResponse.getDefaultInstance());
    }

    @Override
    public void applyFlags(ApplyFlagsRequest request) {}

    @Override
    public void registerResolve(RegisterResolveRequest request) {}

    @Override
    public void flushAllLogs() {
      for (int i = 0; i < flushCount; i++) {
        sink.accept(WriteFlagLogsRequest.getDefaultInstance());
      }
    }

    @Override
    public void flushAssignLogs() {}

    @Override
    public void close() {}

    @Override
    public String prometheusSnapshot() {
      return "";
    }
  }
}
