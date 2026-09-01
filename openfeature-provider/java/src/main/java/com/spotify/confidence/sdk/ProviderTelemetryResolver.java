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
import java.util.function.Supplier;

/** Owns provider-scoped telemetry above the resolver pool and recovery layers. */
final class ProviderTelemetryResolver implements LocalResolver {
  private final Consumer<WriteFlagLogsRequest> logSink;
  private final Supplier<long[]> flushCounterDrain;
  private final Sdk sdk;
  private final Map<String, String> labels;
  private final LocalResolver delegate;
  private boolean initSent;

  ProviderTelemetryResolver(
      Consumer<WriteFlagLogsRequest> logSink,
      Sdk sdk,
      Map<String, String> labels,
      Function<Consumer<WriteFlagLogsRequest>, LocalResolver> innerFactory) {
    this(logSink, () -> new long[] {0, 0}, sdk, labels, innerFactory);
  }

  ProviderTelemetryResolver(
      Consumer<WriteFlagLogsRequest> logSink,
      Supplier<long[]> flushCounterDrain,
      Sdk sdk,
      Map<String, String> labels,
      Function<Consumer<WriteFlagLogsRequest>, LocalResolver> innerFactory) {
    this.logSink = logSink;
    this.flushCounterDrain = flushCounterDrain;
    this.sdk = sdk;
    this.labels = Map.copyOf(labels);
    this.delegate = innerFactory.apply(this::writeLogs);
  }

  private synchronized void writeLogs(WriteFlagLogsRequest request) {
    WriteFlagLogsRequest outgoing = initSent ? request : addInitTelemetry(request);
    outgoing = addFlushCounters(outgoing);
    logSink.accept(outgoing);
    initSent = true;
  }

  private WriteFlagLogsRequest addFlushCounters(WriteFlagLogsRequest request) {
    final long[] counters = flushCounterDrain.get();
    if (counters[0] == 0 && counters[1] == 0) {
      return request;
    }
    return request.toBuilder()
        .setTelemetryData(
            request.getTelemetryData().toBuilder()
                .setFlushSucceeded((int) counters[0])
                .setFlushFailed((int) counters[1])
                .build())
        .build();
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
