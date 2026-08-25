import fs from 'node:fs/promises';
import { ConfidenceServerProviderLocal, ProviderOptions } from './ConfidenceServerProviderLocal';
import { WasmResolver } from './WasmResolver';
import { EventWasmTracker } from './EventWasmTracker';
import { LocalResolver } from './LocalResolver';
import type { EventTracker } from './EventWasmTracker';
export type { MaterializationStore } from './materialization';
export type { SnapshotConfig } from './ConfidenceServerProviderLocal';

let resolver: Promise<LocalResolver> | null = null;
let eventTracker: Promise<EventTracker> | null = null;

export interface ProviderOptionsExt extends ProviderOptions {
  wasmPath?: string;
  /**
   * Set to false to disable event tracking. When true (the default), the
   * provider loads the bundled event engine WASM and enables OpenFeature
   * track() support.
   */
  enableEventTracking?: boolean;
}

export function createConfidenceServerProvider({
  wasmPath,
  enableEventTracking,
  ...options
}: ProviderOptionsExt): ConfidenceServerProviderLocal {
  if (!resolver) {
    resolver = createResolver(wasmPath ?? require.resolve('./confidence_resolver.wasm'));
  }
  if (enableEventTracking !== false && !eventTracker) {
    eventTracker = createEventTracker(require.resolve('./confidence_event_engine.wasm'));
  }
  // The provider awaits eventTracker during initialize(), so passing the
  // pending promise straight through is safe — assigning it after construction
  // would be read too late and silently disable event tracking.
  return new ConfidenceServerProviderLocal(resolver, {
    ...options,
    ...(eventTracker ? { eventTracker } : {}),
  });
}

async function createResolver(wasmPath: string): Promise<LocalResolver> {
  const buffer = await fs.readFile(wasmPath);
  const module = await WebAssembly.compile(buffer as BufferSource);
  return new WasmResolver(module);
}

async function createEventTracker(wasmPath: string): Promise<EventTracker> {
  const buffer = await fs.readFile(wasmPath);
  const module = await WebAssembly.compile(buffer as BufferSource);
  return new EventWasmTracker(module);
}
