import { readSharedMemoryLimits } from './wasm-imports';
import { GuestInstance, type HostFunctions } from './guest-instance';

export interface GuestInstancePoolOptions {
  /**
   * Upper bound on simultaneously existing instances. Suspended resolves pin
   * their instance, so the pool sizes by concurrency, not by state size.
   * Default 32; revisit with real artifacts and benchmarks.
   */
  maxInstances?: number;
  /** Instances instantiated eagerly. Default 1 (warms the first request). */
  warmInstances?: number;
}

interface Waiter {
  resolve(instance: GuestInstance): void;
  reject(error: Error): void;
}

/**
 * Pool of guest instances sharing one memory: one-exclusive-holder semantics.
 *
 * An acquired instance is pinned to its holder until released — a suspended
 * resolve keeps its instance until resumed or discarded, so a concurrent call
 * leases a different instance. Instances of the same module are runtime-equal;
 * there is no primary instance.
 */
export class GuestInstancePool {
  static async create(
    wasmBytes: Uint8Array<ArrayBuffer>,
    hostFunctions: HostFunctions,
    options?: GuestInstancePoolOptions,
  ): Promise<GuestInstancePool> {
    const limits = readSharedMemoryLimits(wasmBytes);
    const memory = new WebAssembly.Memory({ initial: limits.initial, maximum: limits.maximum, shared: true });
    const module = await WebAssembly.compile(wasmBytes);
    return new GuestInstancePool(module, memory, hostFunctions, options);
  }

  private readonly maxInstances: number;
  private idle: GuestInstance[] = [];
  private readonly held = new Set<GuestInstance>();
  private readonly waiters: Waiter[] = [];
  private closed = false;
  private idleWhenClosed: (() => void)[] = [];

  private constructor(
    private readonly module: WebAssembly.Module,
    private readonly sharedMemory: WebAssembly.Memory,
    private readonly hostFunctions: HostFunctions,
    options?: GuestInstancePoolOptions,
  ) {
    const maxInstances = options?.maxInstances ?? 32;
    const warmInstances = options?.warmInstances ?? 1;
    if (!Number.isInteger(maxInstances) || maxInstances < 1) {
      throw new Error(`maxInstances must be an integer >= 1, got: ${maxInstances}`);
    }
    if (!Number.isInteger(warmInstances) || warmInstances < 0 || warmInstances > maxInstances) {
      throw new Error(`warmInstances must be an integer in [0, maxInstances], got: ${warmInstances}`);
    }
    this.maxInstances = maxInstances;
    for (let i = 0; i < warmInstances; i++) {
      this.idle.push(this.instantiate());
    }
  }

  /** The one shared memory every instance of this pool imports. */
  get memory(): WebAssembly.Memory {
    return this.sharedMemory;
  }

  /** Current number of existing (live) instances. */
  get liveCount(): number {
    return this.held.size + this.idle.length;
  }

  /** Current number of instances held by callers. */
  get heldCount(): number {
    return this.held.size;
  }

  /**
   * Lease an idle instance, instantiating a new one while below
   * `maxInstances`, or waiting for a release at the cap.
   */
  async acquire(): Promise<GuestInstance> {
    if (this.closed) throw new Error('GuestInstancePool is destroyed');
    const idleInstance = this.idle.pop();
    if (idleInstance) {
      this.held.add(idleInstance);
      return idleInstance;
    }
    if (this.held.size + this.waiters.length < this.maxInstances) {
      const instance = this.instantiate();
      this.held.add(instance);
      return instance;
    }
    return new Promise<GuestInstance>((resolve, reject) => {
      this.waiters.push({ resolve, reject });
    });
  }

  /** Return a leased instance. Rejects double release. */
  release(instance: GuestInstance): void {
    if (!this.held.delete(instance)) {
      throw new Error('release() called on an instance that is not held');
    }
    if (this.closed) {
      instance.destroyInstance();
      if (this.held.size === 0) this.notifyClosed();
      return;
    }
    const waiter = this.waiters.shift();
    if (waiter) {
      this.held.add(instance);
      waiter.resolve(instance);
      return;
    }
    this.idle.push(instance);
  }

  /**
   * Flush logs on an idle instance if one exists. Creates nothing, waits for
   * nothing: buffers also flush when they fill during resolves, so skipping
   * when busy is fine.
   */
  tryFlushLogsIdle(): boolean {
    const idleInstance = this.idle.pop();
    if (!idleInstance) return false;
    try {
      idleInstance.flushLogs();
    } finally {
      this.idle.push(idleInstance);
    }
    return true;
  }

  /**
   * Stop issuing instances, wait for every holder to release, then destroy
   * every instance. The shared memory is released with the last reference.
   */
  async destroy(signal?: AbortSignal): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    for (const waiter of this.waiters.splice(0)) {
      waiter.reject(new Error('GuestInstancePool destroyed while waiting for an instance'));
    }
    for (const idleInstance of this.idle.splice(0)) {
      idleInstance.destroyInstance();
    }
    if (this.held.size > 0) {
      await new Promise<void>((resolve, reject) => {
        this.idleWhenClosed.push(resolve);
        signal?.addEventListener(
          'abort',
          () => reject(new Error(`GuestInstancePool.destroy aborted with ${this.held.size} instance(s) still held`)),
          { once: true },
        );
      });
    }
  }

  private instantiate(): GuestInstance {
    return new GuestInstance(this.module, this.sharedMemory, this.hostFunctions);
  }

  private notifyClosed(): void {
    for (const resolve of this.idleWhenClosed.splice(0)) resolve();
  }
}
