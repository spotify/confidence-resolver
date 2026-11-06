import { ConfidenceServerProviderLocal, ProviderOptions, validateEncryptionKey } from './ConfidenceServerProviderLocal';
import { EventWasmTracker } from './EventWasmTracker';
import type { EventTracker } from './EventWasmTracker';
export type { MaterializationStore } from './materialization';
export type { SnapshotConfig } from './ConfidenceServerProviderLocal';

// @ts-expect-error - wasm imported as data URL via bundler (configured in tsdown.config.ts)
import eventWasmDataUrl from '../../../wasm/confidence_event_engine.wasm';

let eventTracker: Promise<EventTracker> | null = null;

export type ProviderOptionsExt = ProviderOptions;

export function createConfidenceServerProvider(options: ProviderOptions): ConfidenceServerProviderLocal {
  validateEncryptionKey(options.encryptionKey);
  if (!eventTracker) {
    eventTracker = createEventTracker();
  }
  return new ConfidenceServerProviderLocal({ ...options, eventTracker });
}

async function createEventTracker(): Promise<EventTracker> {
  const module = await WebAssembly.compileStreaming(fetch(eventWasmDataUrl));
  return new EventWasmTracker(module);
}
