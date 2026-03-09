package com.spotify.confidence.sdk;

import io.micrometer.core.instrument.Counter;
import io.micrometer.core.instrument.MeterRegistry;
import io.micrometer.core.instrument.Timer;
import java.util.concurrent.TimeUnit;
import java.util.function.Supplier;

/**
 * Registers Confidence resolver metrics with a Micrometer {@link MeterRegistry}.
 *
 * <p>Requires {@code io.micrometer:micrometer-core} on the classpath. This class is only
 * instantiated when the user explicitly provides a {@link MeterRegistry} via {@link
 * OpenFeatureLocalResolveProvider#registerMetrics(MeterRegistry)}.
 *
 * <p><strong>Exposed metrics:</strong>
 *
 * <ul>
 *   <li>{@code confidence.resolver.wasm.memory_bytes} — Gauge: total WASM linear memory in bytes
 *       across all pool instances.
 *   <li>{@code confidence.resolver.resolve} — Timer: flag resolution latency and count.
 *   <li>{@code confidence.resolver.state.updates} — Counter: number of resolver state refreshes.
 * </ul>
 */
class ProviderMetrics {
  private final Timer resolveTimer;
  private final Counter stateUpdateCounter;
  // Strong reference to prevent GC — Micrometer gauges hold WeakReferences to their target.
  @SuppressWarnings("FieldCanBeLocal")
  private final Supplier<Long> wasmMemoryRef;

  ProviderMetrics(MeterRegistry registry, Supplier<Long> wasmMemoryBytesSupplier) {
    this.wasmMemoryRef = wasmMemoryBytesSupplier;
    registry.gauge("confidence.resolver.wasm.memory_bytes", wasmMemoryRef, Supplier::get);
    this.resolveTimer =
        Timer.builder("confidence.resolver.resolve")
            .description("Flag resolution latency")
            .register(registry);
    this.stateUpdateCounter =
        Counter.builder("confidence.resolver.state.updates")
            .description("Resolver state refresh count")
            .register(registry);
  }

  void recordResolve(long durationNanos) {
    resolveTimer.record(durationNanos, TimeUnit.NANOSECONDS);
  }

  void recordStateUpdate() {
    stateUpdateCounter.increment();
  }
}
