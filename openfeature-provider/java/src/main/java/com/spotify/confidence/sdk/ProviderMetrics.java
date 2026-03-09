package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.TelemetryData;
import io.micrometer.core.instrument.Counter;
import io.micrometer.core.instrument.MeterRegistry;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Supplier;

/**
 * Registers Confidence resolver metrics with a Micrometer {@link MeterRegistry}.
 *
 * <p>Metrics are sourced from the WASM resolver's own telemetry, extracted via {@link
 * TelemetryData} deltas on each log flush. This avoids double-tracking — the same data sent to the
 * Confidence backend is exposed locally as Prometheus metrics.
 *
 * <p>Requires {@code io.micrometer:micrometer-core} on the classpath. This class is only
 * instantiated when the user explicitly provides a {@link MeterRegistry} via {@link
 * OpenFeatureLocalResolveProvider#registerMetrics(Object)}.
 *
 * <p><strong>Exposed metrics:</strong>
 *
 * <ul>
 *   <li>{@code confidence.resolver.wasm.memory_bytes} — Gauge: total WASM linear memory in bytes
 *       across all pool instances.
 *   <li>{@code confidence.resolver.resolve.latency.sum} — Counter: cumulative resolve latency in
 *       microseconds.
 *   <li>{@code confidence.resolver.resolve.latency.count} — Counter: total number of resolves.
 *   <li>{@code confidence.resolver.resolve.count} — Counter(s) tagged by {@code reason}: per-reason
 *       resolve counts (e.g. MATCH, NO_SEGMENT_MATCH, ERROR).
 * </ul>
 */
class ProviderMetrics {
  private final MeterRegistry registry;
  private final Counter latencySum;
  private final Counter latencyCount;
  private final Map<String, Counter> resolveCountByReason = new ConcurrentHashMap<>();

  // Strong reference to prevent GC — Micrometer gauges hold WeakReferences to their target.
  @SuppressWarnings("FieldCanBeLocal")
  private final Supplier<Long> wasmMemoryRef;

  ProviderMetrics(MeterRegistry registry, Supplier<Long> wasmMemoryBytesSupplier) {
    this.registry = registry;
    this.wasmMemoryRef = wasmMemoryBytesSupplier;
    registry.gauge("confidence.resolver.wasm.memory_bytes", wasmMemoryRef, Supplier::get);
    this.latencySum =
        Counter.builder("confidence.resolver.resolve.latency.sum")
            .description("Cumulative resolve latency (microseconds)")
            .baseUnit("microseconds")
            .register(registry);
    this.latencyCount =
        Counter.builder("confidence.resolver.resolve.latency.count")
            .description("Total number of resolves")
            .register(registry);
  }

  void recordTelemetry(TelemetryData telemetry) {
    if (telemetry.hasResolveLatency()) {
      latencySum.increment(telemetry.getResolveLatency().getSum());
      latencyCount.increment(telemetry.getResolveLatency().getCount());
    }
    for (TelemetryData.ResolveRate rate : telemetry.getResolveRateList()) {
      if (rate.getCount() > 0) {
        String reason = rate.getReason().name();
        resolveCountByReason
            .computeIfAbsent(
                reason,
                r ->
                    Counter.builder("confidence.resolver.resolve.count")
                        .description("Resolve count by reason")
                        .tag("reason", r)
                        .register(registry))
            .increment(rate.getCount());
      }
    }
  }
}
