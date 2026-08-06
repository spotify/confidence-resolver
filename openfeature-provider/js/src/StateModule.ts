import {
  ResolveFlagsRequest,
  ResolveProcessResponse,
  ApplyRequest,
  EvaluateRequest,
  Materialization,
  Materialization_Record,
  ResolutionDetails,
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
  /**
   * Hands the payload back to the module. Idempotent; iteration afterwards
   * throws. Not a plain free(): the payload allocation carries ownership data
   * for the parts beyond its length, and only ack_logs releases those too.
   */
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
    this.exports.ack_logs(packSlice(this.ptr, this.len));
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
  ack_logs: (payload: bigint) => void;
  evaluate: (response: bigint, request: bigint) => bigint;
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

function varint(value: number): number[] {
  const bytes: number[] = [];
  let rest = value;
  while (rest > 0x7f) {
    bytes.push((rest & 0x7f) | 0x80);
    rest >>>= 7;
  }
  bytes.push(rest);
  return bytes;
}

/**
 * A resolve response left in module memory, so `evaluate` can read it in
 * place. The module borrows the buffer — evaluate any number of flag keys
 * against one handle — and the host frees it with close().
 *
 * Holding a handle across an await is safe: a concurrent call may grow module
 * memory, which detaches JS views but never moves an allocation, so the
 * pointer stays valid. A handle does NOT survive a state update, since that
 * builds a new module with its own memory.
 */
export class ResolveHandle {
  private closed = false;

  constructor(private readonly module: StateModule, private readonly slice: bigint) {}

  /** Decodes a copy of the borrowed response; the handle stays open. */
  decode(): ResolveProcessResponse {
    this.check();
    return ResolveProcessResponse.decode(this.module.view(this.slice));
  }

  /** Evaluates one flag key against the borrowed response. */
  evaluate(request: EvaluateRequest): ResolutionDetails {
    this.check();
    return this.module.evaluate(this.slice, request);
  }

  /** Releases the response buffer. Idempotent; use afterwards throws. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.module.release(this.slice);
  }

  private check(): void {
    if (this.closed) throw new Error('resolve handle used after close');
  }
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
   * the response's materializationToRead lists the records the flags would
   * need, and flags that need them resolve with MATERIALIZATION_NOT_SUPPORTED.
   */
  resolve(request: ResolveFlagsRequest, materializations?: Materialization_Record[]): ResolveHandle {
    const reqSlice = this.writeToModule(ResolveFlagsRequest.encode(request).finish());
    const matSlice = this.encodeMaterializations(materializations);
    return new ResolveHandle(this, this.wrapInEnvelope(this.exports.resolve_flags(reqSlice, matSlice)));
  }

  /**
   * First half of a two-phase resolve: the handle decodes to either a final
   * response, or a suspended process with the materialization records the
   * host must fetch. A suspended process holds module memory until resumed or
   * discarded — independently of the returned handle.
   */
  resolveStart(request: ResolveFlagsRequest): ResolveHandle {
    const reqSlice = this.writeToModule(ResolveFlagsRequest.encode(request).finish());
    return new ResolveHandle(this, this.exports.resolve_process_start(reqSlice));
  }

  /** Completes a suspended resolve with the fetched records, consuming the process id. */
  resolveResume(processId: number, materializations: Materialization_Record[]): ResolveHandle {
    const matSlice = this.encodeMaterializations(materializations);
    return new ResolveHandle(this, this.wrapInEnvelope(this.exports.resolve_process_resume(processId, matSlice)));
  }

  /** Abandons a suspended process and frees its context. */
  resolveDiscard(processId: number): void {
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

  /**
   * Evaluates one flag key against a borrowed response. Called through
   * {@link ResolveHandle}, which owns the response lifetime.
   */
  evaluate(response: bigint, request: EvaluateRequest): ResolutionDetails {
    const reqSlice = this.writeToModule(EvaluateRequest.encode(request).finish());
    return ResolutionDetails.decode(this.readFromModule(this.exports.evaluate(response, reqSlice)));
  }

  /** Copies a borrowed slice out of module memory without freeing it. */
  view(slice: bigint): Uint8Array {
    if (slice === 0n) return new Uint8Array(0);
    const { ptr, len } = unpackSlice(slice);
    return new Uint8Array(this.exports.memory.buffer, ptr, len).slice();
  }

  /** Frees a slice the host owns. */
  release(slice: bigint): void {
    if (slice === 0n) return;
    this.exports.free(unpackSlice(slice).ptr);
  }

  // resolve_flags and resolve_process_resume return a bare ResolveFlagsResponse,
  // but evaluate takes the ResolveProcessResponse envelope, and the two are not
  // distinguishable on the wire (both length-delimited field 1). Prepend the
  // `resolved` tag + length in module memory rather than round-tripping the
  // bytes through the host.
  private wrapInEnvelope(bare: bigint): bigint {
    if (bare === 0n) return 0n;
    const { ptr, len } = unpackSlice(bare);
    const header = [0x0a, ...varint(len)];
    const size = header.length + len;
    // alloc may grow memory, so take the view after it
    const envPtr = this.exports.alloc(size);
    if (envPtr === 0) throw new Error('wasm alloc returned null');
    const memory = new Uint8Array(this.exports.memory.buffer);
    memory.set(header, envPtr);
    memory.copyWithin(envPtr + header.length, ptr, ptr + len);
    this.exports.free(ptr);
    return packSlice(envPtr, size);
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
