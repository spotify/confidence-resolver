package com.spotify.confidence.sdk.internal.engine;

import com.spotify.confidence.flags.resolver.v1.ResolveFlagsRequest;
import com.spotify.confidence.flags.resolver.v1.ResolveFlagsResponse;
import com.spotify.confidence.flags.wasm.v1.EvaluateRequest;
import com.spotify.confidence.flags.wasm.v1.ResolutionDetails;
import com.spotify.confidence.sdk.internal.cdn.CdnException;
import com.spotify.confidence.sdk.internal.cdn.CdnFetcher;
import com.spotify.confidence.sdk.internal.cdn.NotModifiedException;
import com.spotify.confidence.sdk.internal.cdn.TerminalCdnException;
import com.spotify.confidence.sdk.internal.host.DedupeFilter;
import com.spotify.confidence.sdk.internal.host.Filter;
import com.spotify.confidence.sdk.internal.host.HostConfig;
import com.spotify.confidence.sdk.internal.host.InvalidModuleException;
import com.spotify.confidence.sdk.internal.host.MaterializationStore;
import com.spotify.confidence.sdk.internal.host.Module;
import com.spotify.confidence.sdk.internal.host.Resolver;
import com.spotify.confidence.sdk.internal.ingest.LogShipper;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import java.util.concurrent.locks.ReentrantReadWriteLock;

/**
 * Owns the long-lived parts of a provider: the module fetcher, the log shipper, the dedupe filters,
 * and the current module generation. It swaps generations as new modules are published.
 *
 * <p>Port of the Go provider's internal/engine; that file is the reference for semantics, including
 * the swap-without-dropping-evaluations rules.
 */
public final class Engine {

  public static final String MODULE_PROFILE = "OPENFEATURE_MT_1";

  /** Filter sizes, as in the Java host state-to-wasm builds with. */
  private static final int APPLY_FILTER_PAGES = 8;

  /**
   * Runs generation retirements. A retirement parks until the generation's in-flight evaluations
   * finish, so it must never occupy the common pool, which the host application depends on.
   */
  private static final java.util.concurrent.ExecutorService RETIRE =
      java.util.concurrent.Executors.newCachedThreadPool(
          r -> {
            Thread thread = new Thread(r, "confidence-engine-retire");
            thread.setDaemon(true);
            return thread;
          });

  private static final int SCHEMA_FILTER_PAGES = 2;

  private final EngineConfig cfg;
  private final CdnFetcher fetcher;
  private final LogShipper shipper;
  private final Filter applyFilter;
  private final Filter schemaFilter;

  private final AtomicReference<Generation> current = new AtomicReference<>();
  private final AtomicBoolean closed = new AtomicBoolean();
  private final AtomicBoolean notified = new AtomicBoolean();
  private final ConcurrentLinkedQueue<CompletableFuture<Void>> retiring =
      new ConcurrentLinkedQueue<>();
  private ScheduledExecutorService loops;

  /**
   * One installed module. Requests hold the read lock for the whole evaluation. To retire a
   * generation, first replace the engine's reference, then take the write lock and never release
   * it: the write acquisition waits for readers, and every later read-lock attempt fails, sending
   * the request back to reload the reference, which by then points elsewhere.
   */
  static final class Generation {

    final ReentrantReadWriteLock lock = new ReentrantReadWriteLock();
    final Resolver resolver;
    final String etag;

    Generation(Resolver resolver, String etag) {
      this.resolver = resolver;
      this.etag = etag;
    }
  }

  /** Creates an engine. It doesn't touch the network; call {@link #start(Duration)}. */
  public Engine(EngineConfig cfg) {
    this.cfg = cfg;
    this.fetcher =
        new CdnFetcher(
            cfg.clientSecret,
            cfg.encryptionKey,
            cfg.cdnBaseUrl,
            cfg.resolverBaseUrl,
            MODULE_PROFILE,
            cfg.httpClient);
    switch (cfg.exposure) {
      case ALL -> this.applyFilter = Filter.neverSeen();
      case NONE -> this.applyFilter = Filter.alwaysSeen();
      default -> this.applyFilter = new DedupeFilter(APPLY_FILTER_PAGES);
    }
    this.schemaFilter = new DedupeFilter(SCHEMA_FILTER_PAGES);
    this.shipper =
        new LogShipper(
            cfg.ingestBaseUrl,
            cfg.httpClient,
            0,
            t -> cfg.logger.warn("flag log delivery failed", t));
  }

  /**
   * Installs the first module, retrying until the timeout, then starts polling and timed log
   * flushes. A {@link TerminalStartupException} means the engine won't become ready and is not
   * polling. A {@link StartupTimeoutException} means the first module didn't arrive in time: the
   * engine keeps polling, and calls {@code onReady} once one is installed.
   */
  public void start(Duration initTimeout) {
    RuntimeException lastError = null;
    long deadline = System.nanoTime() + initTimeout.toNanos();
    while (true) {
      try {
        // Bound each attempt by the remaining deadline, as Go bounds the start fetch with its
        // context, so initialize() returns within the budget even against a stalling CDN.
        load(Duration.ofNanos(Math.max(1, deadline - System.nanoTime())));
        notified.set(true); // start reports readiness itself
        break;
      } catch (TerminalStartupException e) {
        throw e;
      } catch (RuntimeException e) {
        lastError = e;
        cfg.logger.warn("loading resolver module failed; retrying", e);
        if (Thread.currentThread().isInterrupted()) {
          throw e;
        }
        long remaining = deadline - System.nanoTime();
        if (remaining <= 0) {
          startLoops();
          throw new StartupTimeoutException(
              "no resolver module within "
                  + initTimeout
                  + " (serving defaults, still "
                  + "polling)",
              lastError);
        }
        sleep(Duration.ofNanos(Math.min(remaining, cfg.retryInterval.toNanos())));
      }
    }
    startLoops();
  }

  private void startLoops() {
    // Two threads, as the Go engine runs poll and flush on separate goroutines: one slow
    // fetch must not stall timed log flushing.
    loops =
        Executors.newScheduledThreadPool(
            2,
            r -> {
              Thread thread = new Thread(r, "confidence-engine-loops");
              thread.setDaemon(true);
              return thread;
            });
    loops.scheduleAtFixedRate(
        this::pollTick,
        cfg.pollInterval.toMillis(),
        cfg.pollInterval.toMillis(),
        TimeUnit.MILLISECONDS);
    loops.scheduleAtFixedRate(
        this::flushTick,
        cfg.flushInterval.toMillis(),
        cfg.flushInterval.toMillis(),
        TimeUnit.MILLISECONDS);
  }

  private void pollTick() {
    try {
      load();
    } catch (RuntimeException e) {
      cfg.logger.warn("updating resolver module failed; keeping the current one", e);
    }
  }

  private void flushTick() {
    flush();
  }

  // ---------------------------------------------------------------- evaluation

  /** Resolves req and evaluates evals against the current module. */
  public List<ResolutionDetails> evaluate(
      ResolveFlagsRequest req, List<EvaluateRequest> evals, MaterializationStore store) {
    Generation g = acquire();
    try {
      return g.resolver.evaluate(req, evals, store);
    } finally {
      g.lock.readLock().unlock();
    }
  }

  /** Resolves req against the current module and returns its full response. */
  public ResolveFlagsResponse resolve(ResolveFlagsRequest req, MaterializationStore store) {
    Generation g = acquire();
    try {
      return g.resolver.resolve(req, store);
    } finally {
      g.lock.readLock().unlock();
    }
  }

  /** Forwards a serialized ApplyFlagsRequest to the current module. */
  public void applyFlags(byte[] request) {
    Generation g = acquire();
    try {
      g.resolver.applyFlags(request);
    } finally {
      g.lock.readLock().unlock();
    }
  }

  /** Renders the current module's cumulative telemetry ("" before the first module). */
  public String metrics(boolean openMetrics) {
    Generation g;
    try {
      g = acquire();
    } catch (NotReadyException | EngineClosedException e) {
      return "";
    }
    try {
      return g.resolver.metrics(openMetrics);
    } finally {
      g.lock.readLock().unlock();
    }
  }

  /** The current generation, for tests; never held. */
  Generation currentGeneration() {
    return current.get();
  }

  private Generation acquire() {
    while (true) {
      Generation g = current.get();
      if (g == null) {
        if (closed.get()) {
          throw new EngineClosedException();
        }
        throw new NotReadyException();
      }
      // One of the rare correct uses of tryLock: failure means g is being retired and has
      // already been replaced, so reloading makes progress.
      if (g.lock.readLock().tryLock()) {
        return g;
      }
    }
  }

  // ---------------------------------------------------------------- module loading

  /** Fetches the module (default timeout) and, if it changed, installs a new generation. */
  private void load() {
    load(Duration.ofSeconds(30));
  }

  /** Fetches the module and, if it changed, installs it as a new generation. */
  private void load(Duration fetchTimeout) {
    CdnFetcher.FetchedModule m;
    try {
      m = fetcher.fetch(fetchTimeout);
    } catch (NotModifiedException e) {
      // The steady state after a module installs: nothing changed, nothing to do.
      return;
    } catch (TerminalCdnException e) {
      throw new TerminalStartupException("module CDN rejected the credential", e);
    } catch (CdnException e) {
      throw e; // retryable
    }
    String token = m.logToken();
    Resolver resolver;
    try {
      resolver =
          new Resolver(
              Module.parse(m.wasm()),
              new HostConfig.Builder()
                  .instances(cfg.instances)
                  .maxMemoryBytes(cfg.maxMemoryBytes)
                  .applyFilter(applyFilter)
                  .schemaFilter(schemaFilter)
                  .interpreter(cfg.interpreter)
                  .writeLogs(
                      b -> {
                        if (!shipper.enqueue(b, token)) {
                          cfg.logger.warn("flag log queue full; dropping a batch");
                        }
                      })
                  .reportFailure(t -> cfg.logger.warn("resolver host callback failed", t))
                  .reportTrap(
                      t ->
                          cfg.logger.error(
                              "module call aborted; the generation's state may be inconsistent", t))
                  .build());
    } catch (InvalidModuleException e) {
      throw new TerminalStartupException("the fetched module is invalid", e);
    }
    try {
      resolver.warm();
    } catch (RuntimeException e) {
      resolver.close();
      throw new IllegalStateException("warming up module: " + e.getMessage(), e);
    }
    if (closed.get()) {
      resolver.close();
      return;
    }
    Generation old = current.getAndSet(new Generation(resolver, m.etag()));
    fetcher.accept(m.etag());
    if (old != null) {
      retireAsync(old);
    } else if (!notified.getAndSet(true)) {
      cfg.onReady.run();
    }
  }

  /** Retires g (which must already be unpublished) on a background thread. */
  private void retireAsync(Generation g) {
    CompletableFuture<Void> future = CompletableFuture.runAsync(() -> retire(g), RETIRE);
    retiring.add(future);
    future.thenRun(() -> retiring.remove(future));
  }

  /** Drains g, flushes its logs, and closes it. */
  private void retire(Generation g) {
    g.lock.writeLock().lock(); // never released
    try {
      g.resolver.close();
    } catch (RuntimeException e) {
      cfg.logger.warn("closing retired resolver module", e);
    }
  }

  // ---------------------------------------------------------------- close

  /** Flushes the current generation's logs, as the timed flush does. */
  void flush() {
    Generation g;
    try {
      g = acquire();
    } catch (NotReadyException | EngineClosedException e) {
      return;
    }
    try {
      g.resolver.flushLogs();
    } catch (RuntimeException e) {
      cfg.logger.warn("flushing flag logs", e);
    } finally {
      g.lock.readLock().unlock();
    }
  }

  /**
   * Stops polling, retires the current module (flushing its logs), and waits for the log queue to
   * drain, bounded by the timeout. Retiring waits for in-flight evaluations, which may be in a slow
   * materialization read; if the timeout ends first, the rest of the cleanup runs once they finish,
   * and false is returned. New evaluations fail with {@link EngineClosedException} either way.
   */
  public boolean close(Duration timeout) {
    if (!closed.compareAndSet(false, true)) {
      return true;
    }
    long deadline = System.nanoTime() + timeout.toNanos();
    if (loops != null) {
      // Drain, don't interrupt: shutdownNow() would abort an in-flight module call (Endive
      // checks the thread flag at every call), possibly half-flushing logs. The ticks exit on
      // the closed flag, and an in-flight fetch is bounded by its request timeout.
      loops.shutdown();
      try {
        loops.awaitTermination(Math.max(0, deadline - System.nanoTime()), TimeUnit.NANOSECONDS);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
      }
    }
    Generation g = current.getAndSet(null);
    if (g != null) {
      retireAsync(g);
    }
    // The filters and the shipper outlive every generation: modules link to the filters and
    // flush their logs into the shipper when retired.
    try {
      CompletableFuture.allOf(retiring.toArray(new CompletableFuture[0]))
          .get(Math.max(0, deadline - System.nanoTime()), TimeUnit.NANOSECONDS);
    } catch (Exception e) {
      return false; // evaluations still in flight; cleanup continues in the background
    }
    Duration remaining = Duration.ofNanos(Math.max(0, deadline - System.nanoTime()));
    return shipper.close(remaining);
  }

  private void sleep(Duration duration) {
    try {
      TimeUnit.NANOSECONDS.sleep(duration.toNanos());
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
    }
  }
}
