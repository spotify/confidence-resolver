package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;
import java.util.Optional;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.locks.ReentrantReadWriteLock;
import java.util.function.Consumer;
import java.util.function.Function;
import java.util.function.Supplier;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Pool layer: manages N {@link LocalResolver} slots for concurrent access. Read operations ({@code
 * resolveProcess}, {@code applyFlags}) acquire a read lock on a slot via round-robin with tryLock
 * fallback. Maintenance operations acquire write locks sequentially across all slots.
 */
class PooledResolver implements LocalResolver {
  private static final Logger logger = LoggerFactory.getLogger(PooledResolver.class);

  private final Slot[] slots;
  private final AtomicLong roundRobin = new AtomicLong(0);

  static int getNumInstances() {
    final var defaultNumberOfInstances = Runtime.getRuntime().availableProcessors();
    return Optional.ofNullable(System.getProperty("CONFIDENCE_NUMBER_OF_WASM_INSTANCES"))
        .or(() -> Optional.ofNullable(System.getenv("CONFIDENCE_NUMBER_OF_WASM_INSTANCES")))
        .map(Integer::parseInt)
        .orElse(defaultNumberOfInstances);
  }

  PooledResolver(int size, Supplier<LocalResolver> factory) {
    // +1 slot like Go implementation for extra headroom
    this.slots = new Slot[size + 1];
    for (int i = 0; i < slots.length; i++) {
      slots[i] = new Slot(factory.get());
    }
    logger.info("Created PooledResolver with {} slots", slots.length);
  }

  @Override
  public CompletionStage<ResolveProcessResponse> resolveProcess(ResolveProcessRequest request) {
    return withReadSlot(lr -> lr.resolveProcess(request));
  }

  @Override
  public void applyFlags(ApplyFlagsRequest request) {
    withReadSlotVoid(lr -> lr.applyFlags(request));
  }

  @Override
  public void setResolverState(byte[] state, String accountId) {
    maintenance(lr -> lr.setResolverState(state, accountId));
  }

  @Override
  public void flushAllLogs() {
    maintenance(LocalResolver::flushAllLogs);
  }

  @Override
  public void flushAssignLogs() {
    maintenance(LocalResolver::flushAssignLogs);
  }

  @Override
  public void close() {
    maintenance(LocalResolver::close);
  }

  @Override
  public long getWasmMemoryBytes() {
    long total = 0;
    for (Slot slot : slots) {
      total += slot.resolver.getWasmMemoryBytes();
    }
    return total;
  }

  /**
   * Acquires a read lock on a slot via round-robin with fallback. Used for resolve and apply
   * operations which can run concurrently.
   */
  private <T> T withReadSlot(Function<LocalResolver, T> fn) {
    final int n = slots.length;
    while (true) {
      final int idx = (int) (roundRobin.incrementAndGet() % n);
      final Slot slot = slots[Math.abs(idx)];
      if (slot.rwLock.readLock().tryLock()) {
        try {
          return fn.apply(slot.resolver);
        } finally {
          slot.rwLock.readLock().unlock();
        }
      }
    }
  }

  private void withReadSlotVoid(Consumer<LocalResolver> fn) {
    withReadSlot(
        lr -> {
          fn.accept(lr);
          return null;
        });
  }

  /**
   * Acquires write locks sequentially across all slots. Used for maintenance operations that must
   * run exclusively (state update, flush, close).
   */
  private void maintenance(Consumer<LocalResolver> fn) {
    for (int i = 0; i < slots.length; i++) {
      final Slot slot = slots[i];
      slot.rwLock.writeLock().lock();
      try {
        fn.accept(slot.resolver);
      } catch (RuntimeException e) {
        logger.error("Maintenance operation failed on slot {}", i, e);
      } finally {
        slot.rwLock.writeLock().unlock();
      }
    }
  }

  private static class Slot {
    final LocalResolver resolver;
    final ReentrantReadWriteLock rwLock = new ReentrantReadWriteLock();

    Slot(LocalResolver resolver) {
      this.resolver = resolver;
    }
  }
}
