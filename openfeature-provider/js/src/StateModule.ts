import {
  ResolveFlagsRequest,
  ResolveFlagsResponse,
  ResolveProcessResponse,
  ApplyRequest,
  Materialization,
  Materialization_Record,
  WriteLogsPayload,
} from './proto/state_module/api';
import { getLogger } from './logger';

const logger = getLogger('state-module');

/** Return codes from the module's apply_flags export */
export enum ApplyResult {
  OK = 0,
  INVALID_REQUEST = 1,
  UNKNOWN_FLAG = 2,
  DUPLICATE_FLAG = 3,
}

export interface StateModuleOptions {
  /** Wall clock as unix epoch milliseconds. Defaults to Date.now. */
  currentTime?: () => number;
  /**
   * Receives telemetry emitted by the module: chunks that concatenate to one
   * serialized WriteFlagLogsRequest. The receiver owns the payload and must
   * call free() when done, or the payload leaks in module memory.
   * Defaults to dropping the logs.
   */
  onLogs?: (logs: LogChunks) => void;
}

/**
 * A telemetry payload as zero-copy chunks of module memory.
 *
 * Re-iterable until free(): each iteration mints fresh views into the
 * module's current memory. A yielded view is only valid for the current
 * synchronous stretch — any call into the module (including from other tasks
 * after an await) may grow its memory, which detaches earlier views. After an
 * await, re-iterate instead of reusing views; if a sink queues buffers
 * (socket.write, fetch), copy the data into it. After free() the module may
 * reuse the underlying memory.
 */
export interface LogChunks extends Iterable<Uint8Array> {
  /** Releases the payload back to the module. Idempotent; iteration afterwards throws. */
  free(): void;
  /** Concatenates all chunks into one host-owned buffer. */
  copy(): Uint8Array;
}

class LogPayload implements LogChunks {
  // lazily decoded so that a bare free() never touches the parts
  private parts: number[] | null = null;
  private freed = false;

  constructor(
    private readonly exports: StateModuleExports,
    private readonly ptr: number,
    private readonly len: number,
  ) {}

  *[Symbol.iterator](): IterableIterator<Uint8Array> {
    if (this.parts === null) {
      if (this.freed) throw new Error('log chunks accessed after free');
      this.parts = WriteLogsPayload.decode(new Uint8Array(this.exports.memory.buffer, this.ptr, this.len)).parts;
    }
    for (const part of this.parts) {
      if (this.freed) throw new Error('log chunks accessed after free');
      // parts are fixed64 slices decoded as JS numbers — exact only below 2^53,
      // i.e. for parts shorter than 2 MiB, which the log buffer stays well under
      const partPtr = part % 0x100000000;
      const partLen = Math.floor(part / 0x100000000);
      // mint the view from the current buffer: a memory.grow since the last
      // yield detaches old views but leaves contents and offsets intact
      yield new Uint8Array(this.exports.memory.buffer, partPtr, partLen);
    }
  }

  copy(): Uint8Array {
    const views = [...this];
    const out = new Uint8Array(views.reduce((total, view) => total + view.length, 0));
    let offset = 0;
    for (const view of views) {
      out.set(view, offset);
      offset += view.length;
    }
    return out;
  }

  free(): void {
    if (this.freed) return;
    this.freed = true;
    this.exports.free(this.ptr);
  }
}

interface StateModuleExports {
  memory: WebAssembly.Memory;
  alloc: (size: number) => number;
  free: (ptr: number) => void;
  resolve_flags: (request: bigint, materializations: bigint) => bigint;
  resolve_process_start: (request: bigint) => bigint;
  resolve_process_resume: (process: number, materializations: bigint) => bigint;
  resolve_process_discard: (process: number) => void;
  apply_flags: (request: bigint) => number;
  flush_logs: () => void;
}

// byte ranges cross the wasm boundary as a single i64: pointer in the low
// 32 bits, length in the high 32; 0 is the null/empty slice
function packSlice(ptr: number, len: number): bigint {
  return (BigInt(len) << 32n) | BigInt(ptr);
}

function unpackSlice(slice: bigint): { ptr: number; len: number } {
  return {
    ptr: Number(slice & 0xffffffffn),
    len: Number((slice >> 32n) & 0xffffffffn),
  };
}

/**
 * Native binding for a compiled state module: a wasm module produced by
 * compiling a resolver state, exposing resolve/apply/log entry points that
 * exchange state_module protos.
 */
export class StateModule {
  private readonly exports: StateModuleExports;

  constructor(module: WebAssembly.Module, options: StateModuleOptions = {}) {
    const currentTime = options.currentTime ?? Date.now;
    const onLogs =
      options.onLogs ??
      (logs => {
        logger.debug('dropping logs');
        logs.free();
      });
    const instance = new WebAssembly.Instance(module, {
      env: {
        current_time: (): bigint => BigInt(currentTime()),
        write_logs: (payload: bigint): void => {
          if (payload === 0n) return;
          const { ptr, len } = unpackSlice(payload);
          onLogs(new LogPayload(this.exports, ptr, len));
        },
      },
    });
    this.exports = instance.exports as unknown as StateModuleExports;
  }

  /**
   * Single-shot resolve. Omitting `materializations` runs in discovery mode:
   * the response's materializationToRead lists the records the flags would need.
   */
  resolveFlags(request: ResolveFlagsRequest, materializations?: Materialization_Record[]): ResolveFlagsResponse {
    const reqSlice = this.writeToModule(ResolveFlagsRequest.encode(request).finish());
    const matSlice = this.encodeMaterializations(materializations);
    return ResolveFlagsResponse.decode(this.readFromModule(this.exports.resolve_flags(reqSlice, matSlice)));
  }

  /**
   * First half of a two-phase resolve: returns either a final response, or a
   * suspended process with the materialization records the host must fetch.
   * A suspended process holds module memory until resumed or discarded.
   */
  resolveProcessStart(request: ResolveFlagsRequest): ResolveProcessResponse {
    const reqSlice = this.writeToModule(ResolveFlagsRequest.encode(request).finish());
    return ResolveProcessResponse.decode(this.readFromModule(this.exports.resolve_process_start(reqSlice)));
  }

  /** Completes a suspended resolve with the fetched records, consuming the process handle. */
  resolveProcessResume(processId: number, materializations: Materialization_Record[]): ResolveFlagsResponse {
    const matSlice = this.encodeMaterializations(materializations);
    return ResolveFlagsResponse.decode(this.readFromModule(this.exports.resolve_process_resume(processId, matSlice)));
  }

  /** Abandons a suspended process and frees its context. */
  resolveProcessDiscard(processId: number): void {
    this.exports.resolve_process_discard(processId);
  }

  /** Deferred apply: emits FlagAssigned events for an earlier resolve. */
  applyFlags(request: ApplyRequest): ApplyResult {
    const reqSlice = this.writeToModule(ApplyRequest.encode(request).finish());
    return this.exports.apply_flags(reqSlice);
  }

  /** Serializes pending telemetry and delivers it through the onLogs callback. */
  flushLogs(): void {
    this.exports.flush_logs();
  }

  // copy bytes into module memory via alloc, return the packed slice;
  // the module frees request buffers itself
  private writeToModule(data: Uint8Array): bigint {
    const exports = this.exports;
    const ptr = exports.alloc(data.length);
    if (ptr === 0) throw new Error('wasm alloc returned null');
    new Uint8Array(exports.memory.buffer, ptr, data.length).set(data);
    return packSlice(ptr, data.length);
  }

  // copy response bytes out of module memory (defensive copy), then free
  private readFromModule(slice: bigint): Uint8Array {
    if (slice === 0n) return new Uint8Array(0);
    const { ptr, len } = unpackSlice(slice);
    const copy = new Uint8Array(this.exports.memory.buffer, ptr, len).slice();
    this.exports.free(ptr);
    return copy;
  }

  private encodeMaterializations(records?: Materialization_Record[]): bigint {
    if (!records) return 0n;
    return this.writeToModule(Materialization.encode({ records }).finish());
  }
}
