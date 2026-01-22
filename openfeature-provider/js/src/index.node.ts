import fs from 'node:fs/promises';
import { ConfidenceServerProviderLocal, ProviderOptions } from './ConfidenceServerProviderLocal';
import { WasmResolver } from './WasmResolver';
import { LocalResolver } from './LocalResolver';
export type { MaterializationStore } from './materialization';

let resolver: Promise<LocalResolver> | null = null;
export interface ProviderOptionsExt extends ProviderOptions {
  wasmPath?: string;
}

export function createConfidenceServerProvider({
  wasmPath,
  ...options
}: ProviderOptionsExt): ConfidenceServerProviderLocal {
  if (!resolver) {
    resolver = createResolver(wasmPath ?? require.resolve('./confidence_resolver.wasm'));
  }
  return new ConfidenceServerProviderLocal(resolver, options);
}

async function createResolver(wasmPath: string): Promise<LocalResolver> {
  const buffer = await fs.readFile(wasmPath);
  const module = await WebAssembly.compile(buffer as BufferSource);
  return new WasmResolver(module);
}
