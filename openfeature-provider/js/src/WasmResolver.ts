import { ABSENT_SLICE, packSlice, unpackSlice, type PackedSlice } from './slice';

/** A log payload delivered through the `env.write_logs` host import. */
export interface GuestLogPayload {
  /**
   * Copy of the payload bytes. The underlying memory is only valid inside the
   * callback: read it once and keep the copy.
   */
  bytes(): Uint8Array;
  /**
   * Copy of a packed slice's bytes from module memory (e.g. one of
   * `WriteLogsPayload.parts`). Reading is allowed inside the callback; never
   * free or ack individual parts — only the payload itself is acknowledged.
   */
  readSlice(packed: PackedSlice): Uint8Array;
  /** Release the payload in the module. Call exactly once, even on failure. */
  ack(): void;
}

/**
 * Host functions the compiled client module imports.
 *
 * `current_time` must be cheap and thread-safe; `random_seed` is called once
 * per instance and must never throw; `write_logs` runs on the calling thread,
 * must never throw and must never re-enter the resolver.
 */
export interface HostFunctions {
  /** Wall-clock milliseconds since the epoch. */
  currentTime(): bigint;
  /** Independently generated random value, once per instance. */
  randomSeed(): bigint;
  /** Log payload delivery: copy what you need, forward out of band, ack exactly once. */
  writeLogs(payload: GuestLogPayload): void;
  /** Exposure de-duplication: return true when the fingerprint was already seen. */
  applyFilterSeen(fingerprint: bigint): boolean;
  /** Schema de-duplication: return true when the fingerprint was already seen. */
  schemaFilterSeen(fingerprint: bigint): boolean;
  /** Called when a host function violates its contract (e.g. `writeLogs` threw). */
  onHostFunctionError?(where: string, error: unknown): void;
}

/** Optional exports the module may or may not provide. */
export interface GuestCapabilities {
  prometheusSnapshot: boolean;
  observeClientResolveLatency: boolean;
  evaluate: boolean;
}

const REQUIRED_EXPORTS = [
  'alloc',
  'free',
  'resolve_process_start',
  'resolve_process_resume',
  'resolve_process_discard',
  'apply_flags',
  'flush_logs',
  'ack_logs',
  'allocated_bytes',
  'destroy_instance',
] as const;

const OPTIONAL_EXPORTS = ['metrics_snapshot', 'observe_client_resolve_latency', 'evaluate'] as const;

type GuestExports = {
  alloc(size: number): number;
  free(ptr: number): void;
  resolve_process_start(request: PackedSlice): PackedSlice;
  resolve_process_resume(process: number, materializations: PackedSlice): PackedSlice;
  resolve_process_discard(process: number): void;
  apply_flags(request: PackedSlice): number;
  flush_logs(): void;
  ack_logs(payload: PackedSlice): void;
  allocated_bytes(): number;
  metrics_snapshot?(format: number): PackedSlice;
  observe_client_resolve_latency?(clientIndex: number, micros: bigint): void;
  evaluate?(response: PackedSlice, request: PackedSlice): PackedSlice;
  destroy_instance(): void;
};

/**
 * One instance of a compiled client module, bound to the shared memory and
 * the host functions, speaking the raw ABI with correct slice ownership:
 *
 * - request slices are allocated with `alloc`, passed in, and taken over by
 *   the module (never freed by the host);
 * - response slices are module-owned; the host reads them and frees them;
 * - `evaluate` borrows its response envelope: keep it alive across calls and
 *   free it once when done;
 * - log payloads are acknowledged through `ack_logs`, never freed.
 */
export class WasmResolver {
  readonly capabilities: GuestCapabilities;
  private readonly exports: GuestExports;

  constructor(
    module: WebAssembly.Module,
    private readonly memory: WebAssembly.Memory,
    private readonly hostFunctions: HostFunctions,
  ) {
    if (!(memory.buffer instanceof SharedArrayBuffer)) {
      throw new Error('memory must be shared (WebAssembly.Memory created with shared: true)');
    }
    const moduleExports = new Set(WebAssembly.Module.exports(module).map(e => e.name));
    for (const name of REQUIRED_EXPORTS) {
      if (!moduleExports.has(name)) {
        throw new Error(`Module is missing required export "${name}"`);
      }
    }
    this.capabilities = {
      prometheusSnapshot: moduleExports.has('metrics_snapshot'),
      observeClientResolveLatency: moduleExports.has('observe_client_resolve_latency'),
      evaluate: moduleExports.has('evaluate'),
    };

    const instance = new WebAssembly.Instance(module, this.buildImports());
    this.exports = instance.exports as unknown as GuestExports;
  }

  // --- raw ABI ---------------------------------------------------------------

  /** Start a resolve. Returns the response envelope slice; free it when done. */
  resolveProcessStart(request: Uint8Array): PackedSlice {
    return this.exports.resolve_process_start(this.writeSlice(request));
  }

  /** Resume a suspended process. The suspended envelope is spent; free the new one. */
  resolveProcessResume(processId: number, materializations: Uint8Array): PackedSlice {
    return this.exports.resolve_process_resume(processId, this.writeSlice(materializations));
  }

  /** Drop a suspended process without resuming it. */
  resolveProcessDiscard(processId: number): void {
    this.exports.resolve_process_discard(processId);
  }

  /** Apply previously resolved flags. Returns a status code (0 = ok). */
  applyFlags(request: Uint8Array): number {
    return this.exports.apply_flags(this.writeSlice(request));
  }

  /** Flush buffered logs now (delivered through the `writeLogs` host function). */
  flushLogs(): void {
    this.exports.flush_logs();
  }

  /** Text metrics snapshot. `format`: 0 = Prometheus, 1 = OpenMetrics. */
  prometheusSnapshot(format: 0 | 1): Uint8Array {
    this.assertCapability('metrics_snapshot', this.capabilities.prometheusSnapshot);
    return this.readAndFreeSlice(this.exports.metrics_snapshot!(format));
  }

  /** Report one resolve's latency for the client the request authenticated as. */
  observeClientResolveLatency(clientIndex: number, micros: bigint): void {
    this.assertCapability('observe_client_resolve_latency', this.capabilities.observeClientResolveLatency);
    this.exports.observe_client_resolve_latency!(clientIndex, micros);
  }

  /**
   * Evaluate one flag key against a default, using the module's own
   * evaluation step. `responseEnvelope` is the slice returned by
   * `resolveProcessStart`/`resolveProcessResume`: it is borrowed, may be
   * evaluated against repeatedly, and must be freed by the caller once.
   */
  evaluate(responseEnvelope: PackedSlice, request: Uint8Array): Uint8Array {
    this.assertCapability('evaluate', this.capabilities.evaluate);
    return this.readAndFreeSlice(this.exports.evaluate!(responseEnvelope, this.writeSlice(request)));
  }

  /** Tear this instance down. Must be called before retiring it. */
  destroyInstance(): void {
    this.exports.destroy_instance();
  }

  /** Bytes currently allocated by the module. */
  allocatedBytes(): number {
    return this.exports.allocated_bytes();
  }

  // --- slice helpers ---------------------------------------------------------

  /** Allocate `bytes` in the module's memory and pack them as a request slice. */
  writeSlice(bytes: Uint8Array): PackedSlice {
    if (bytes.length === 0) return ABSENT_SLICE;
    const ptr = this.exports.alloc(bytes.length);
    if (ptr === 0) {
      throw new Error(`Module alloc(${bytes.length}) failed`);
    }
    new Uint8Array(this.memory.buffer).set(bytes, ptr);
    return packSlice(ptr, bytes.length);
  }

  /** Copy a module-owned slice out of wasm memory. The module still owns it. */
  readSlice(packed: PackedSlice): Uint8Array {
    const { ptr, len } = unpackSlice(packed);
    if (len === 0) return new Uint8Array(0);
    return new Uint8Array(this.memory.buffer).slice(ptr, ptr + len);
  }

  /** Copy a module-owned slice out and free it in one step. */
  readAndFreeSlice(packed: PackedSlice): Uint8Array {
    const bytes = this.readSlice(packed);
    this.free(unpackSlice(packed).ptr);
    return bytes;
  }

  /** Free a module-owned block. Never call this on a log payload. */
  free(ptr: number): void {
    this.exports.free(ptr);
  }

  // --- host imports ----------------------------------------------------------

  private buildImports(): WebAssembly.Imports {
    const hostFunctions = this.hostFunctions;
    return {
      host: { memory: this.memory },
      env: {
        current_time: (): bigint => hostFunctions.currentTime(),
        random_seed: (): bigint => hostFunctions.randomSeed(),
        write_logs: (payload: PackedSlice): void => {
          let acked = false;
          const handle: GuestLogPayload = {
            bytes: () => this.readSlice(payload),
            readSlice: packed => this.readSlice(packed),
            ack: () => {
              if (acked) return;
              acked = true;
              this.exports.ack_logs(payload);
            },
          };
          try {
            hostFunctions.writeLogs(handle);
          } catch (error) {
            // Host callbacks must never throw into the module; record and move on.
            hostFunctions.onHostFunctionError?.('writeLogs', error);
          } finally {
            // Guarantee the "exactly once" ack even if the callback forgot.
            handle.ack();
          }
        },
      },
      apply_filter: {
        seen: (fingerprint: bigint): number => (hostFunctions.applyFilterSeen(fingerprint) ? 1 : 0),
      },
      schema_filter: {
        seen: (fingerprint: bigint): number => (hostFunctions.schemaFilterSeen(fingerprint) ? 1 : 0),
      },
    };
  }

  private assertCapability(name: string, present: boolean): void {
    if (!present) {
      throw new Error(`Module does not export "${name}"`);
    }
  }
}
