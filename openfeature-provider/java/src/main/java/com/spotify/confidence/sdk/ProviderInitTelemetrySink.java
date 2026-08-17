package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.Sdk;
import com.spotify.confidence.sdk.flags.resolver.v1.TelemetryData;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import java.util.Map;
import java.util.function.Consumer;

/** Adds provider-init telemetry once, above the resolver pool and recovery layers. */
final class ProviderInitTelemetrySink implements Consumer<WriteFlagLogsRequest> {
  private final Consumer<WriteFlagLogsRequest> delegate;
  private final Sdk sdk;
  private final Map<String, String> labels;
  private boolean sent;

  ProviderInitTelemetrySink(
      Consumer<WriteFlagLogsRequest> delegate, Sdk sdk, Map<String, String> labels) {
    this.delegate = delegate;
    this.sdk = sdk;
    this.labels = Map.copyOf(labels);
  }

  @Override
  public synchronized void accept(WriteFlagLogsRequest request) {
    final WriteFlagLogsRequest outgoing;
    if (sent) {
      outgoing = request;
    } else {
      outgoing =
          request.toBuilder()
              .setTelemetryData(
                  request.getTelemetryData().toBuilder()
                      .setSdk(sdk)
                      .addProviderInitRate(
                          TelemetryData.ProviderInitRate.newBuilder()
                              .setCount(1)
                              .putAllLabels(labels)
                              .build())
                      .build())
              .build();
    }

    delegate.accept(outgoing);
    sent = true;
  }

  synchronized void emitIfPending() {
    if (!sent) {
      accept(WriteFlagLogsRequest.getDefaultInstance());
    }
  }
}
