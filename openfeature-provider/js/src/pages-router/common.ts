import { OpenFeature } from '@openfeature/server-sdk';
import type { ConfidenceProvider } from './types';

const PROVIDER_NAME = 'ConfidenceServerProviderLocal';

/**
 * Resolves the OpenFeature provider and narrows it to a Confidence provider.
 * Returns `null` if the registered provider is not a `ConfidenceServerProviderLocal`
 * — the user is expected to register one (typically in `instrumentation.ts`).
 *
 * Mirrors `isConfidenceServerProviderLocal` from the App Router integration.
 */
export function getConfidenceProvider(providerName?: string): ConfidenceProvider | null {
  const provider = providerName ? OpenFeature.getProvider(providerName) : OpenFeature.getProvider();
  if (provider?.metadata?.name !== PROVIDER_NAME) return null;
  return provider as ConfidenceProvider;
}
