package com.spotify.confidence.sdk.internal.host;

import java.util.function.Consumer;

/** Configuration for a {@link Resolver}. Zero values select defaults. */
public final class HostConfig {

  /** Bounds concurrent calls into the module. Default 1. */
  public final int instances;

  /**
   * Caps the shared memory, in bytes. Default 64 MiB, and never above the module's declared
   * maximum.
   */
  public final long maxMemoryBytes;

  /**
   * Receives each serialized WriteFlagLogsRequest. It owns the array. It must not block or call
   * back into the resolver.
   */
  public final Consumer<byte[]> writeLogs;

  /** Answers the module's apply-filter dedup import. Default: every key unseen. */
  public final Filter applyFilter;

  /** Answers the module's schema-filter dedup import. Default: every key unseen. */
  public final Filter schemaFilter;

  /** Receives errors raised inside host callbacks. */
  public final Consumer<Throwable> reportFailure;

  /**
   * Called once, with the error, when a trap or an interrupt aborts a module call midway. The
   * module's shared state may then be inconsistent: later calls fail with {@link
   * PoisonedException}, calls in flight on other instances may never return, and close abandons the
   * module without calling into it. It must not block.
   */
  public final Consumer<Throwable> onPoison;

  /** Runs the module on the interpreter engine instead of the runtime compiler. */
  public final boolean interpreter;

  private HostConfig(Builder b) {
    this.instances = b.instances;
    this.maxMemoryBytes = b.maxMemoryBytes;
    this.writeLogs = b.writeLogs;
    this.applyFilter = b.applyFilter;
    this.schemaFilter = b.schemaFilter;
    this.reportFailure = b.reportFailure;
    this.onPoison = b.onPoison;
    this.interpreter = b.interpreter;
  }

  public static final class Builder {

    private int instances = 1;
    private long maxMemoryBytes = 64L << 20;
    private Consumer<byte[]> writeLogs = b -> {};
    private Filter applyFilter = Filter.neverSeen();
    private Filter schemaFilter = Filter.neverSeen();
    private Consumer<Throwable> reportFailure = t -> {};
    private Consumer<Throwable> onPoison = t -> {};
    private boolean interpreter = false;

    /** Instances bounds concurrent calls into the module. Default 1. */
    public Builder instances(int instances) {
      this.instances = instances;
      return this;
    }

    /** Caps the shared memory. Default 64 MiB. */
    public Builder maxMemoryBytes(long maxMemoryBytes) {
      this.maxMemoryBytes = maxMemoryBytes;
      return this;
    }

    /** Receives each serialized WriteFlagLogsRequest; must not block. */
    public Builder writeLogs(Consumer<byte[]> writeLogs) {
      this.writeLogs = writeLogs;
      return this;
    }

    /** Sets the apply-filter. Default: every key unseen. */
    public Builder applyFilter(Filter applyFilter) {
      this.applyFilter = applyFilter;
      return this;
    }

    /** Sets the schema-filter. Default: every key unseen. */
    public Builder schemaFilter(Filter schemaFilter) {
      this.schemaFilter = schemaFilter;
      return this;
    }

    /** Receives errors raised inside host callbacks. */
    public Builder reportFailure(Consumer<Throwable> reportFailure) {
      this.reportFailure = reportFailure;
      return this;
    }

    /** Receives traps and interrupts that aborted a module call midway. */
    public Builder onPoison(Consumer<Throwable> onPoison) {
      this.onPoison = onPoison;
      return this;
    }

    /** Runs the module on the interpreter engine. */
    public Builder interpreter(boolean interpreter) {
      this.interpreter = interpreter;
      return this;
    }

    public HostConfig build() {
      return new HostConfig(this);
    }
  }
}
