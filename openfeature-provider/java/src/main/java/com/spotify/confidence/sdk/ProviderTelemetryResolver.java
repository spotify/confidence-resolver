package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.RegisterResolveRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.Sdk;
import com.spotify.confidence.sdk.flags.resolver.v1.TelemetryData;
import com.spotify.confidence.sdk.flags.resolver.v1.WriteFlagLogsRequest;
import java.util.Map;
import java.util.concurrent.CompletionStage;
import java.util.function.Consumer;
import java.util.function.Function;

/** Owns provider-scoped telemetry above the resolver pool and recovery layers. */
final class ProviderTelemetryResolver implements LocalResolver {
  private final Consumer<WriteFlagLogsRequest> logSink;
  private final Sdk sdk;
  private final Map<String, String> labels;
  private final LocalResolver delegate;
  private boolean initSent;

  ProviderTelemetryResolver(
      Consumer<WriteFlagLogsRequest> logSink,
      Sdk sdk,
      Map<String, String> labels,
      Function<Consumer<WriteFlagLogsRequest>, LocalResolver> innerFactory) {
    this.logSink = logSink;
    this.sdk = sdk;
    this.labels = Map.copyOf(labels);
    this.delegate = innerFactory.apply(this::writeLogs);
  }

  private synchronized void writeLogs(WriteFlagLogsRequest request) {
    final WriteFlagLogsRequest outgoing = initSent ? request : addInitTelemetry(request);
    logSink.accept(outgoing);
    initSent = true;
  }

  private synchronized void emitInitIfPending() {
    if (initSent) {
      return;
    }
    logSink.accept(addInitTelemetry(WriteFlagLogsRequest.getDefaultInstance()));
    initSent = true;
  }

  private WriteFlagLogsRequest addInitTelemetry(WriteFlagLogsRequest request) {
    return request.toBuilder()
        .setTelemetryData(
            request.getTelemetryData().toBuilder()
                .setSdk(sdk)
                .clearProviderInitRate()
                .addProviderInitRate(
                    TelemetryData.ProviderInitRate.newBuilder()
                        .setCount(1)
                        .putAllLabels(labels)
                        .build())
                .build())
        .build();
  }

  @Override
  public void setResolverState(byte[] state, String accountId, Sdk sdk) {
    delegate.setResolverState(state, accountId, sdk);
  }

  @Override
  public CompletionStage<ResolveProcessResponse> resolveProcess(ResolveProcessRequest request) {
    return delegate.resolveProcess(request);
  }

  @Override
  public void applyFlags(ApplyFlagsRequest request) {
    delegate.applyFlags(request);
  }

  @Override
  public void registerResolve(RegisterResolveRequest request) {
    delegate.registerResolve(request);
  }

  @Override
  public void flushAllLogs() {
    delegate.flushAllLogs();
  }

  @Override
  public void flushAssignLogs() {
    delegate.flushAssignLogs();
  }

  @Override
  public void close() {
    try {
      delegate.close();
    } finally {
      emitInitIfPending();
    }
  }

  @Override
  public String prometheusSnapshot() {
    return delegate.prometheusSnapshot();
  }
}
