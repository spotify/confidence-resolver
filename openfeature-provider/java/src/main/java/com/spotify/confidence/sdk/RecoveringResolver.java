package com.spotify.confidence.sdk;

import com.dylibso.chicory.wasm.ChicoryException;
import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Supplier;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Recovery layer: wraps a {@link LocalResolver} and recreates it in the background if it throws a
 * {@link ChicoryException} (WASM runtime failures like {@code _unreachable_}). Caches the last
 * successful state so the new instance can be reinitialized.
 */
class RecoveringResolver implements LocalResolver {
  private static final Logger logger = LoggerFactory.getLogger(RecoveringResolver.class);

  private record StateRecord(byte[] state, String accountId) {}

  private final Supplier<LocalResolver> factory;
  private final AtomicReference<LocalResolver> current = new AtomicReference<>();
  private final AtomicBoolean broken = new AtomicBoolean(false);
  private final AtomicReference<StateRecord> lastState = new AtomicReference<>();

  RecoveringResolver(Supplier<LocalResolver> factory) {
    this.factory = factory;
    this.current.set(factory.get());
  }

  private void startRecreate() {
    final Thread t =
        new Thread(
            () -> {
              try {
                final LocalResolver old = current.get();
                final LocalResolver newResolver = factory.get();
                final StateRecord cached = lastState.get();
                if (cached != null) {
                  newResolver.setResolverState(cached.state(), cached.accountId());
                }
                current.set(newResolver);
                if (old != null) {
                  try {
                    old.close();
                  } catch (Exception e) {
                    // best-effort close of old instance
                  }
                }
              } catch (Exception e) {
                logger.error("Failed to recreate resolver", e);
              } finally {
                broken.set(false);
              }
            });
    t.setDaemon(true);
    t.setName("recovering-resolver-recreate");
    t.start();
  }

  private void handleFailure(String opName, ChicoryException e) {
    if (broken.compareAndSet(false, true)) {
      logger.warn("Resolver panicked during {}, starting background recreation", opName, e);
      startRecreate();
    }
  }

  @Override
  public void setResolverState(byte[] state, String accountId) {
    final StateRecord record = new StateRecord(state, accountId);
    try {
      current.get().setResolverState(state, accountId);
      lastState.set(record);
    } catch (ChicoryException e) {
      lastState.set(record);
      handleFailure("setResolverState", e);
      throw e;
    }
  }

  @Override
  public CompletionStage<ResolveProcessResponse> resolveProcess(ResolveProcessRequest request) {
    try {
      return current.get().resolveProcess(request);
    } catch (ChicoryException e) {
      handleFailure("resolveProcess", e);
      throw e;
    }
  }

  @Override
  public void applyFlags(ApplyFlagsRequest request) {
    try {
      current.get().applyFlags(request);
    } catch (ChicoryException e) {
      handleFailure("applyFlags", e);
      throw e;
    }
  }

  @Override
  public void flushAllLogs() {
    try {
      current.get().flushAllLogs();
    } catch (ChicoryException e) {
      handleFailure("flushAllLogs", e);
    }
  }

  @Override
  public void flushAssignLogs() {
    try {
      current.get().flushAssignLogs();
    } catch (ChicoryException e) {
      handleFailure("flushAssignLogs", e);
    }
  }

  @Override
  public void close() {
    // During close, do NOT recreate on failure — we are shutting down.
    try {
      final LocalResolver lr = current.get();
      if (lr != null) {
        lr.close();
      }
    } catch (RuntimeException e) {
      logger.warn("Resolver panicked during close, not recreating", e);
    }
  }
}
