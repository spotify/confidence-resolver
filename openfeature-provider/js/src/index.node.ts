import fs from 'node:fs/promises';
import { ConfidenceServerProviderLocal, ProviderOptions, validateEncryptionKey } from './ConfidenceServerProviderLocal';
import { EventWasmTracker } from './EventWasmTracker';
import type { EventTracker } from './EventWasmTracker';
export type { MaterializationStore } from './materialization';
export type { SnapshotConfig } from './ConfidenceServerProviderLocal';

let eventTracker: Promise<EventTracker> | null = null;

export interface ProviderOptionsExt extends ProviderOptions {
  /** @deprecated the state resolver WASM is no longer used; accepted and ignored. */
  wasmPath?: string;
}

export function createConfidenceServerProvider({
  wasmPath: _wasmPath,
  ...options
}: ProviderOptionsExt): ConfidenceServerProviderLocal {
  validateEncryptionKey(options.encryptionKey);
  if (!eventTracker) {
    eventTracker = createEventTracker(require.resolve('./confidence_event_engine.wasm'));
  }
  return new ConfidenceServerProviderLocal({ ...options, eventTracker });
}

async function createEventTracker(wasmPath: string): Promise<EventTracker> {
  const buffer = await fs.readFile(wasmPath);
  const module = await WebAssembly.compile(buffer as BufferSource);
  return new EventWasmTracker(module);
}
