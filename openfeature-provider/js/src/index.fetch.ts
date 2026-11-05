import { ConfidenceServerProviderLocal, ProviderOptions, validateEncryptionKey } from './ConfidenceServerProviderLocal';
import { EventWasmTracker } from './EventWasmTracker';
import type { EventTracker } from './EventWasmTracker';
export type { MaterializationStore } from './materialization';
export type { SnapshotConfig } from './ConfidenceServerProviderLocal';

let eventTracker: Promise<EventTracker> | null = null;

export interface ProviderOptionsExt extends ProviderOptions {
  /** @deprecated the state resolver WASM is no longer used; accepted and ignored. */
  wasmUrl?: URL | string;
}

export function createConfidenceServerProvider({
  wasmUrl: _wasmUrl,
  ...options
}: ProviderOptionsExt): ConfidenceServerProviderLocal {
  validateEncryptionKey(options.encryptionKey);
  if (!eventTracker) {
    eventTracker = createEventTracker(new URL('confidence_event_engine.wasm', import.meta.url));
  }
  return new ConfidenceServerProviderLocal({ ...options, eventTracker });
}

async function createEventTracker(wasmUrl: URL | string): Promise<EventTracker> {
  const module = await WebAssembly.compileStreaming(fetch(wasmUrl));
  return new EventWasmTracker(module);
}
