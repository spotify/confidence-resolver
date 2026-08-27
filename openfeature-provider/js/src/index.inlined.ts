import { ConfidenceServerProviderLocal, ProviderOptions } from './ConfidenceServerProviderLocal';
import { EventWasmTracker } from './EventWasmTracker';
import { LocalResolver } from './LocalResolver';
import { WasmResolver } from './WasmResolver';
import type { EventTracker } from './EventWasmTracker';
export type { MaterializationStore } from './materialization';
export type { SnapshotConfig } from './ConfidenceServerProviderLocal';

// @ts-expect-error - wasm imported as data URL via bundler (configured in tsdown.config.ts)
import wasmDataUrl from '../../../wasm/confidence_resolver.wasm';
// @ts-expect-error - wasm imported as data URL via bundler (configured in tsdown.config.ts)
import eventWasmDataUrl from '../../../wasm/confidence_event_engine.wasm';

let resolver: Promise<LocalResolver> | null = null;
let eventTracker: Promise<EventTracker> | null = null;

export type ProviderOptionsExt = ProviderOptions;

export function createConfidenceServerProvider(options: ProviderOptions): ConfidenceServerProviderLocal {
  if (!resolver) {
    resolver = createResolver();
  }
  if (!eventTracker) {
    eventTracker = createEventTracker();
  }
  return new ConfidenceServerProviderLocal(resolver, eventTracker, options);
}

async function createResolver(): Promise<LocalResolver> {
  const module = await WebAssembly.compileStreaming(fetch(wasmDataUrl));
  return new WasmResolver(module);
}

async function createEventTracker(): Promise<EventTracker> {
  const module = await WebAssembly.compileStreaming(fetch(eventWasmDataUrl));
  return new EventWasmTracker(module);
}
