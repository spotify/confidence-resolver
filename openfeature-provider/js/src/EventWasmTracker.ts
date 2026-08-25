import { BinaryWriter } from '@bufbuild/protobuf/wire';
import { Request, Response } from './proto/confidence/wasm/messages';
import { TrackEventRequest, FlushEventsResponse, Void } from './proto/confidence/events/wasm/v1/wasm_api';
import { getLogger } from './logger';

const logger = getLogger('event-tracker');

type Codec<T> = {
  encode(message: T): BinaryWriter;
  decode(input: Uint8Array): T;
};

const EVENT_EXPORT_FN_NAMES = [
  'wasm_msg_alloc',
  'wasm_msg_free',
  'wasm_msg_guest_track_event',
  'wasm_msg_guest_bounded_flush_events',
] as const;
type EVENT_EXPORT_FN_NAMES = (typeof EVENT_EXPORT_FN_NAMES)[number];

type EventExports = { memory: WebAssembly.Memory } & {
  [K in EVENT_EXPORT_FN_NAMES]: Function;
};

function verifyEventExports(exports: WebAssembly.Exports): asserts exports is EventExports {
  for (const fnName of EVENT_EXPORT_FN_NAMES) {
    if (typeof exports[fnName] !== 'function') {
      throw new Error(`Expected Function export "${fnName}" found ${exports[fnName]}`);
    }
  }
  if (!(exports.memory instanceof WebAssembly.Memory)) {
    throw new Error(`Expected WebAssembly.Memory export "memory", found ${exports.memory}`);
  }
}

export interface EventTracker {
  trackEvent(request: TrackEventRequest): void;
  flushEvents(): FlushEventsResponse;
}

export class UnsafeEventWasmTracker implements EventTracker {
  private exports: EventExports;

  constructor(module: WebAssembly.Module) {
    const { exports } = new WebAssembly.Instance(module, {});
    verifyEventExports(exports);
    this.exports = exports;
  }

  trackEvent(request: TrackEventRequest): void {
    const reqPtr = this.transferRequest(request, TrackEventRequest);
    const resPtr = this.exports.wasm_msg_guest_track_event(reqPtr);
    this.consumeResponse(resPtr, Void);
  }

  flushEvents(): FlushEventsResponse {
    const resPtr = this.exports.wasm_msg_guest_bounded_flush_events(0);
    const { data, error }: Response = this.consume(resPtr, Response);
    if (error) throw new Error(error);
    return FlushEventsResponse.decode(data!);
  }

  private transferRequest<T>(value: T, codec: Codec<T>): number {
    const data = codec.encode(value).finish();
    return this.transfer({ data }, Request);
  }

  private consumeResponse<T>(ptr: number, codec: Codec<T>): T {
    const { data, error }: Response = this.consume(ptr, Response);
    if (error) throw new Error(error);
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
    const res = codec.decode(data.slice());
    this.exports.wasm_msg_free(ptr);
    return res;
  }

  private viewBuffer(ptr: number): Uint8Array {
    const size = new DataView(this.exports.memory.buffer).getUint32(ptr - 4, true);
    return new Uint8Array(this.exports.memory.buffer, ptr, size - 4);
  }
}

export class EventWasmTracker implements EventTracker {
  private delegate: EventTracker;

  constructor(private readonly module: WebAssembly.Module) {
    this.delegate = new UnsafeEventWasmTracker(module);
  }

  trackEvent(request: TrackEventRequest): void {
    try {
      this.delegate.trackEvent(request);
    } catch (error: unknown) {
      if (error instanceof WebAssembly.RuntimeError) {
        // A trap can leave the instance in an undefined state. Reload it and
        // swallow, mirroring how the Go/Python trackers recover.
        logger.error('Event WASM crashed on trackEvent, reloading instance:', error);
        this.delegate = new UnsafeEventWasmTracker(this.module);
        return;
      }
      // Anything else (proto encode failure, a guest-reported error) leaves the
      // instance healthy. Surface it rather than losing it silently — the
      // provider's track() logs it.
      throw error;
    }
  }

  flushEvents(): FlushEventsResponse {
    try {
      return this.delegate.flushEvents();
    } catch (error: unknown) {
      if (error instanceof WebAssembly.RuntimeError) {
        logger.error('Event WASM crashed on flushEvents, reloading instance:', error);
        this.delegate = new UnsafeEventWasmTracker(this.module);
      } else {
        // Never return an empty batch without saying why: the caller cannot
        // otherwise tell a genuine empty flush from a failed one.
        logger.warn('Failed to flush events, dropping this batch:', error);
      }
      return FlushEventsResponse.create({});
    }
  }
}
