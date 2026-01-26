import { ConfidenceServerProviderLocal, ProviderOptions } from './ConfidenceServerProviderLocal';
import { LocalResolver } from './LocalResolver';
import { WasmResolver } from './WasmResolver';
export type { MaterializationStore } from './materialization';

// @ts-expect-error - wasm imported as data URL via bundler (configured in tsdown.config.ts)
import wasmDataUrl from '../../../wasm/confidence_resolver.wasm';

let resolver: Promise<LocalResolver> | null = null;

export type ProviderOptionsExt = ProviderOptions;

export function createConfidenceServerProvider(options: ProviderOptions): ConfidenceServerProviderLocal {
  if (!resolver) {
    resolver = createResolver();
  }
  return new ConfidenceServerProviderLocal(resolver, options);
}

async function createResolver(): Promise<LocalResolver> {
  const module = await WebAssembly.compileStreaming(fetch(wasmDataUrl));
  return new WasmResolver(module);
}