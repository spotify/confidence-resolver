import { BinaryWriter } from '@bufbuild/protobuf/wire';
import { Request, Response, Void, SetResolverStateRequest } from './proto/messages';
import { Timestamp } from './proto/google/protobuf/timestamp';
import { ResolveWithStickyRequest, ResolveWithStickyResponse } from './proto/resolver/api';
import { LocalResolver } from './LocalResolver';
import { getLogger } from './logger';

const logger = getLogger('wasm-resolver');

export type Codec<T> = {
  encode(message: T): BinaryWriter;
  decode(input: Uint8Array): T;
};

const EXPORT_FN_NAMES = [
  'wasm_msg_alloc',
  'wasm_msg_free',
  'wasm_msg_guest_resolve_with_sticky',
  'wasm_msg_guest_set_resolver_state',
  'wasm_msg_guest_bounded_flush_logs',
  'wasm_msg_guest_bounded_flush_assign',
] as const;
type EXPORT_FN_NAMES = (typeof EXPORT_FN_NAMES)[number];

type ResolverExports = { memory: WebAssembly.Memory } & {
  [K in EXPORT_FN_NAMES]: Function;
};

function verifyExports(exports: WebAssembly.Exports): asserts exports is ResolverExports {
  for (const fnName of EXPORT_FN_NAMES) {
    if (typeof exports[fnName] !== 'function') {
      throw new Error(`Expected Function export "${fnName}" found ${exports[fnName]}`);
    }
  }
  if (!(exports.memory instanceof WebAssembly.Memory)) {
    throw new Error(`Expected WebAssembly.Memory export "memory", found ${exports.memory}`);
  }
}

export class UnsafeWasmResolver implements LocalResolver {
  private exports: ResolverExports;
  private flushCount = 0;

  constructor(module: WebAssembly.Module) {
    const imports = {
      wasm_msg: {
        wasm_msg_host_current_time: () => {
          const epochMillisecond = Date.now();
          const seconds = Math.floor(epochMillisecond / 1000);
          const nanos = (epochMillisecond - 1000 * seconds) * 1_000_000;
          const ptr = this.transferRequest({ seconds, nanos }, Timestamp);
          return ptr;
        },
      },
    };
    const { exports } = new WebAssembly.Instance(module, imports);
    verifyExports(exports);
    this.exports = exports;
  }

  resolveWithSticky(request: ResolveWithStickyRequest): ResolveWithStickyResponse {
    const reqPtr = this.transferRequest(request, ResolveWithStickyRequest);
    const resPtr = this.exports.wasm_msg_guest_resolve_with_sticky(reqPtr);
    return this.consumeResponse(resPtr, ResolveWithStickyResponse);
  }

  setResolverState(request: SetResolverStateRequest): void {
    const reqPtr = this.transferRequest(request, SetResolverStateRequest);
    const resPtr = this.exports.wasm_msg_guest_set_resolver_state(reqPtr);
    this.consumeResponse(resPtr, Void);
  }

  flushLogs(): Uint8Array {
    const resPtr = this.exports.wasm_msg_guest_bounded_flush_logs(0);
    const { data, error } = this.consume(resPtr, Response);
    if (error) {
      throw new Error(error);
    }
    return data!;
  }

  flushAssigned(): Uint8Array {
    const resPtr = this.exports.wasm_msg_guest_bounded_flush_assign(0);
    const { data, error } = this.consume(resPtr, Response);
    if (error) {
      throw new Error(error);
    }
    return data!;
  }

  private transferRequest<T>(value: T, codec: Codec<T>): number {
    const data = codec.encode(value).finish();
    return this.transfer({ data }, Request);
  }

  private consumeResponse<T>(ptr: number, codec: Codec<T>): T {
    const { data, error }: Response = this.consume(ptr, Response);
    if (error) {
      throw new Error(error);
    }
    return codec.decode(data!);
  }

  private transfer<T>(data: T, codec: Codec<T>): number {
    const encoded = codec.encode(data).finish();
    const ptr = this.exports.wasm_msg_alloc(encoded.length);
    this.viewBuffer(ptr).set(encoded);
    return ptr;
  }

  private consume<T>(ptr: number, codec: Codec<T>): T {
    const data = this.viewBuffer(ptr);
    // we need this defensive copy cause codec.decode might returns views into the buffer
    const res = codec.decode(data.slice());
    this.free(ptr);
    return res;
  }

  private viewBuffer(ptr: number): Uint8Array {
    const size = new DataView(this.exports.memory.buffer).getUint32(ptr - 4, true);
    const data = new Uint8Array(this.exports.memory.buffer, ptr, size - 4);
    return data;
  }

  private free(ptr: number) {
    this.exports.wasm_msg_free(ptr);
  }
}

export type DelegateFactory = (module: WebAssembly.Module) => LocalResolver;

export const DEFAULT_DELEGATE_FACTORY: DelegateFactory = module => new UnsafeWasmResolver(module);
export class WasmResolver implements LocalResolver {
  private delegate: LocalResolver;
  private currentState?: { state: Uint8Array; accountId: string };
  private bufferedLogs: Uint8Array[] = [];

  constructor(private readonly module: WebAssembly.Module, private delegateFactory = DEFAULT_DELEGATE_FACTORY) {
    this.delegate = delegateFactory(module);
  }

  private reloadInstance(error: unknown) {
    logger.error('Failure calling into wasm:', error);
    try {
      this.bufferedLogs.push(this.delegate.flushLogs());
    } catch (_) {
      logger.error('Failed to flushLogs on error');
    }

    this.delegate = this.delegateFactory(this.module);
    if (this.currentState) {
      this.delegate.setResolverState(this.currentState);
    }
  }

  resolveWithSticky(request: ResolveWithStickyRequest): ResolveWithStickyResponse {
    try {
      return this.delegate.resolveWithSticky(request);
    } catch (error: unknown) {
      if (error instanceof WebAssembly.RuntimeError) {
        this.reloadInstance(error);
      }
      throw error;
    }
  }

  setResolverState(request: SetResolverStateRequest): void {
    this.currentState = request;
    try {
      this.delegate.setResolverState(request);
    } catch (error: unknown) {
      if (error instanceof WebAssembly.RuntimeError) {
        this.reloadInstance(error);
      }
      throw error;
    }
  }

  flushLogs(): Uint8Array {
    try {
      this.bufferedLogs.push(this.delegate.flushLogs());
      const len = this.bufferedLogs.reduce((sum, chunk) => sum + chunk.length, 0);
      const buffer = new Uint8Array(len);
      let offset = 0;
      for (const chunk of this.bufferedLogs) {
        buffer.set(chunk, offset);
        offset += chunk.length;
      }
      this.bufferedLogs.length = 0;
      return buffer;
    } catch (error: unknown) {
      if (error instanceof WebAssembly.RuntimeError) {
        this.reloadInstance(error);
      }
      throw error;
    }
  }

  flushAssigned(): Uint8Array {
    // TODO buffer logs and resend on failure
    return this.delegate.flushAssigned();
  }
}
