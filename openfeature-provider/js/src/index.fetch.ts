import { ConfidenceServerProviderLocal, ProviderOptions } from './ConfidenceServerProviderLocal';
import { EventWasmTracker } from './EventWasmTracker';
import { LocalResolver } from './LocalResolver';
import { WasmResolver } from './WasmResolver';
import type { EventTracker } from './EventWasmTracker';
export type { MaterializationStore } from './materialization';
export type { SnapshotConfig } from './ConfidenceServerProviderLocal';

let resolver: Promise<LocalResolver> | null = null;
let eventTracker: Promise<EventTracker> | null = null;

export interface ProviderOptionsExt extends ProviderOptions {
  wasmUrl?: URL | string;
}

export function createConfidenceServerProvider({
  wasmUrl,
  ...options
}: ProviderOptionsExt): ConfidenceServerProviderLocal {
  if (!resolver) {
    resolver = createResolver(wasmUrl ?? new URL('confidence_resolver.wasm', import.meta.url));
  }
  if (!eventTracker) {
    eventTracker = createEventTracker(new URL('confidence_event_engine.wasm', import.meta.url));
  }
  return new ConfidenceServerProviderLocal(resolver, eventTracker, options);
}

async function createResolver(wasmUrl: URL | string): Promise<LocalResolver> {
  const module = await WebAssembly.compileStreaming(fetch(wasmUrl));
  return new WasmResolver(module);
}

async function createEventTracker(wasmUrl: URL | string): Promise<EventTracker> {
  const module = await WebAssembly.compileStreaming(fetch(wasmUrl));
  return new EventWasmTracker(module);
}
