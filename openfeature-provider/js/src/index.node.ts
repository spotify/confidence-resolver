import fs from 'node:fs/promises';
import { ConfidenceServerProviderLocal, ProviderOptions } from './ConfidenceServerProviderLocal';
import { WasmResolver } from './WasmResolver';
import { EventWasmResolver } from './EventWasmResolver';
import { LocalResolver } from './LocalResolver';
import type { EventResolver } from './EventWasmResolver';
export type { MaterializationStore } from './materialization';
export type { SnapshotConfig } from './ConfidenceServerProviderLocal';

let resolver: Promise<LocalResolver> | null = null;
let eventResolver: Promise<EventResolver> | null = null;

export interface ProviderOptionsExt extends ProviderOptions {
  wasmPath?: string;
  /**
   * Path to confidence_event_engine.wasm. When set, the provider enables
   * OpenFeature track() support and publishes events to the Confidence
   * events API.
   */
  eventWasmPath?: string;
}

export function createConfidenceServerProvider({
  wasmPath,
  eventWasmPath,
  ...options
}: ProviderOptionsExt): ConfidenceServerProviderLocal {
  if (!resolver) {
    resolver = createResolver(wasmPath ?? require.resolve('./confidence_resolver.wasm'));
  }
  if (eventWasmPath && !eventResolver) {
    eventResolver = createEventResolver(eventWasmPath);
  }
  // The provider awaits eventResolver during initialize(), so passing the
  // pending promise straight through is safe — assigning it after construction
  // would be read too late and silently disable event tracking.
  return new ConfidenceServerProviderLocal(resolver, {
    ...options,
    ...(eventResolver ? { eventResolver } : {}),
  });
}

async function createResolver(wasmPath: string): Promise<LocalResolver> {
  const buffer = await fs.readFile(wasmPath);
  const module = await WebAssembly.compile(buffer as BufferSource);
  return new WasmResolver(module);
}

async function createEventResolver(wasmPath: string): Promise<EventResolver> {
  const buffer = await fs.readFile(wasmPath);
  const module = await WebAssembly.compile(buffer as BufferSource);
  return new EventWasmResolver(module);
}
