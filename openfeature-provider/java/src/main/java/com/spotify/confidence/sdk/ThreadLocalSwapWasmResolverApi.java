package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveWithStickyRequest;
import com.spotify.futures.CompletableFutures;
import java.util.ArrayList;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.stream.IntStream;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Pre-initialized resolver instances mapped by thread ID to CPU core count. This eliminates both
 * lock contention and lazy initialization overhead.
 */
class ThreadLocalSwapWasmResolverApi implements ResolverApi {
  private static final Logger logger =
      LoggerFactory.getLogger(ThreadLocalSwapWasmResolverApi.class);
  private final WasmFlagLogger flagLogger;
  private final MaterializationStore materializationStore;

  // Pre-initialized resolver instances mapped by core index
  private final Map<Integer, SwapWasmResolverApi> resolverInstances = new ConcurrentHashMap<>();
  private final int numInstances;
  private volatile boolean initialized = false;
  private final AtomicInteger nextInstanceIndex = new AtomicInteger(0);
  private final ThreadLocal<Integer> threadInstanceIndex =
      new ThreadLocal<>() {
        @Override
        protected Integer initialValue() {
          return nextInstanceIndex.getAndIncrement() % numInstances;
        }
      };

  public ThreadLocalSwapWasmResolverApi(
      WasmFlagLogger flagLogger, MaterializationStore materializationStore) {
    this.flagLogger = flagLogger;
    this.materializationStore = materializationStore;

    this.numInstances = getNumInstances();
  }

  private static int getNumInstances() {
    // Pre-create instances based on CPU core count for optimal performance
    final var defaultNumberOfInstances = Runtime.getRuntime().availableProcessors();

    // Check system property first (allows tests to override), then env var
    return Optional.ofNullable(System.getProperty("CONFIDENCE_NUMBER_OF_WASM_INSTANCES"))
        .or(() -> Optional.ofNullable(System.getenv("CONFIDENCE_NUMBER_OF_WASM_INSTANCES")))
        .map(Integer::parseInt)
        .orElse(defaultNumberOfInstances);
  }

  @Override
  public void init(byte[] state, String accountId) {
    logger.info(
        "Initialized ThreadLocalSwapWasmResolverApi with {} available processors", numInstances);
    final var futures = new ArrayList<CompletableFuture<Void>>(numInstances);

    IntStream.range(0, numInstances)
        .forEach(
            i ->
                futures.add(
                    CompletableFuture.runAsync(
                        () -> {
                          final var instance =
                              new SwapWasmResolverApi(
                                  this.flagLogger, state, accountId, this.materializationStore);
                          instance.init(state, accountId);
                          resolverInstances.put(i, instance);
                        })));
    CompletableFutures.allAsList(futures).join();
    initialized = true;
  }

  @Override
  public boolean isInitialized() {
    return initialized;
  }

  /**
   * Updates state and flushes logs for all pre-initialized resolver instances. This method is
   * typically called by a scheduled task to refresh the resolver state.
   */
  @Override
  public void updateStateAndFlushLogs(byte[] state, String accountId) {
    final var futures =
        resolverInstances.values().stream()
            .map(v -> CompletableFuture.runAsync(() -> v.updateStateAndFlushLogs(state, accountId)))
            .toList();
    CompletableFutures.allAsList(futures).join();
  }

  /**
   * Maps the current thread to a resolver instance using round-robin assignment. Each thread gets
   * assigned to an instance index when first accessed, ensuring even distribution across available
   * instances.
   */
  private SwapWasmResolverApi getResolverForCurrentThread() {
    final int instanceIndex = threadInstanceIndex.get();
    return resolverInstances.get(instanceIndex);
  }

  /** Delegates resolveWithSticky to the assigned SwapWasmResolverApi instance. */
  @Override
  public CompletableFuture<ResolveFlagsResponse> resolveWithSticky(
      ResolveWithStickyRequest request) {
    return getResolverForCurrentThread().resolveWithSticky(request).toCompletableFuture();
  }

  /** Closes all pre-initialized resolver instances and clears the map. */
  @Override
  public void close() {
    resolverInstances.values().forEach(SwapWasmResolverApi::close);
    flagLogger.shutdown();
    resolverInstances.clear();
  }

  /** Delegates applyFlags to the assigned SwapWasmResolverApi instance. */
  @Override
  public void applyFlags(ApplyFlagsRequest request) {
    getResolverForCurrentThread().applyFlags(request);
  }
}
