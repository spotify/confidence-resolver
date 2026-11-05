import { OpenFeature } from '@openfeature/server-sdk';
import { isConfidenceProvider } from '../provider-contract';
import type { ConfidenceProvider } from './types';

/**
 * Resolves the OpenFeature provider and narrows it to a Confidence provider
 * (by structure: `resolve`, `applyFlag`, `evaluate` — the state-based
 * ConfidenceServerProviderLocal or the module-based ConfidenceModuleProvider).
 * Returns `null` if the registered provider does not implement the Confidence
 * bundle API — the user is expected to register one (typically in
 * `instrumentation.ts`).
 *
 * Mirrors the App Router integration's provider check.
 */
export function getConfidenceProvider(providerName?: string): ConfidenceProvider | null {
  const provider = providerName ? OpenFeature.getProvider(providerName) : OpenFeature.getProvider();
  return isConfidenceProvider(provider) ? provider : null;
}
