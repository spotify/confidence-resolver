package com.spotify.confidence.sdk;

import com.spotify.confidence.sdk.flags.resolver.v1.ApplyFlagsRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessRequest;
import com.spotify.confidence.sdk.flags.resolver.v1.ResolveProcessResponse;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Supplier;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Recovery layer: wraps a {@link LocalResolver} and recreates it in the background if it throws a
 * {@link RuntimeException} (the Java equivalent of Go panics from WASM). Caches the last
 * successful state so the new instance can be reinitialized.
 */
class RecoveringResolver implements LocalResolver {
  private static final Logger logger = LoggerFactory.getLogger(RecoveringResolver.class);

  private final Supplier<LocalResolver> factory;
  private final AtomicReference<LocalResolver> current = new AtomicReference<>();
  private final AtomicBoolean broken = new AtomicBoolean(false);
  private final AtomicReference<byte[]> lastState = new AtomicReference<>();
  private final AtomicReference<String> lastAccountId = new AtomicReference<>();

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
                final byte[] state = lastState.get();
                final String accountId = lastAccountId.get();
                if (state != null && accountId != null) {
                  newResolver.setResolverState(state, accountId);
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

  private void handleFailure(String opName, RuntimeException e) {
    if (broken.compareAndSet(false, true)) {
      logger.warn("Resolver panicked during {}, starting background recreation", opName, e);
      startRecreate();
    }
  }

  @Override
  public void setResolverState(byte[] state, String accountId) {
    try {
      current.get().setResolverState(state, accountId);
      lastState.set(state);
      lastAccountId.set(accountId);
    } catch (RuntimeException e) {
      lastState.set(state);
      lastAccountId.set(accountId);
      handleFailure("setResolverState", e);
      throw e;
    }
  }

  @Override
  public ResolveProcessResponse resolveProcess(ResolveProcessRequest request) {
    try {
      return current.get().resolveProcess(request);
    } catch (RuntimeException e) {
      handleFailure("resolveProcess", e);
      throw e;
    }
  }

  @Override
  public void applyFlags(ApplyFlagsRequest request) {
    try {
      current.get().applyFlags(request);
    } catch (RuntimeException e) {
      handleFailure("applyFlags", e);
      throw e;
    }
  }

  @Override
  public void flushAllLogs() {
    try {
      current.get().flushAllLogs();
    } catch (RuntimeException e) {
      handleFailure("flushAllLogs", e);
    }
  }

  @Override
  public void flushAssignLogs() {
    try {
      current.get().flushAssignLogs();
    } catch (RuntimeException e) {
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
