import { StateModuleProvider, type StateModuleProviderOptions } from './StateModuleProvider';

export type { MaterializationStore } from './materialization';
export type { StateModuleProviderOptions } from './StateModuleProvider';
export { StateModuleProvider } from './StateModuleProvider';

/**
 * Creates a Confidence OpenFeature provider.
 *
 * There is no wasm binary to locate: the resolver state is itself a compiled
 * wasm module, fetched at runtime like any other state payload. That is why
 * this entry point is platform-neutral and no `./node` or `./fetch` variant is
 * needed — they existed only to bundle or load `confidence_resolver.wasm`.
 */
export function createConfidenceServerProvider(options: StateModuleProviderOptions): StateModuleProvider {
  return new StateModuleProvider(options);
}
