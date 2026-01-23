import { ConfidenceServerProviderLocal, type ProviderOptions } from './ConfidenceServerProviderLocal';
import type { LocalResolver } from './LocalResolver';
import { WasmResolver } from './WasmResolver';
export type { MaterializationStore } from './materialization';

let resolver: Promise<LocalResolver> | null = null;

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
  return new ConfidenceServerProviderLocal(resolver, options);
}

async function createResolver(wasmUrl: URL | string): Promise<LocalResolver> {
  const module = await WebAssembly.compileStreaming(fetch(wasmUrl));
  return new WasmResolver(module);
}
