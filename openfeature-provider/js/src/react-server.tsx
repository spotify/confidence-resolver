import { OpenFeature, Provider } from '@openfeature/server-sdk';
import type { EvaluationContext } from '@openfeature/server-sdk';
import type { ConfidenceServerProviderLocal } from './ConfidenceServerProviderLocal';
import { ConfidenceClientProvider } from './react-client';

const PROVIDER_NAME = 'ConfidenceServerProviderLocal';

function isConfidenceServerProviderLocal(provider: Provider): provider is ConfidenceServerProviderLocal {
  return provider?.metadata?.name === PROVIDER_NAME;
}

export interface ConfidenceProviderProps {
  /** The evaluation context for flag resolution */
  evalContext: EvaluationContext;
  /** Optional provider name. If not specified, uses the default provider. */
  providerName?: string;
  /** Flag names to resolve. If not specified, resolves all flags. */
  flags?: string[];
  /** Child components */
  children: React.ReactNode;
}

/**
 * Server component that resolves flags and provides them to client components.
 * Must be used with a ConfidenceServerProviderLocal registered with OpenFeature.
 */
export async function ConfidenceProvider({
  evalContext,
  providerName,
  flags = [],
  children,
}: ConfidenceProviderProps): Promise<React.ReactElement> {
  const provider = providerName ? OpenFeature.getProvider(providerName) : OpenFeature.getProvider();

  if (!isConfidenceServerProviderLocal(provider)) {
    throw new Error(
      `ConfidenceProvider requires a ConfidenceServerProviderLocal, but got ${
        provider?.metadata?.name ?? 'undefined'
      }. ` + 'Make sure you have registered the provider with OpenFeature before rendering.',
    );
  }

  const bundle = await provider.resolveFlagBundle(evalContext, ...flags);

  async function applyFlag(flagName: string): Promise<void> {
    'use server';

    const serverProvider = providerName ? OpenFeature.getProvider(providerName) : OpenFeature.getProvider();

    if (!isConfidenceServerProviderLocal(serverProvider)) {
      throw new Error('ConfidenceServerProviderLocal not found');
    }

    serverProvider.applyFlag(bundle.resolveToken, flagName);
  }

  return (
    <ConfidenceClientProvider bundle={bundle} apply={applyFlag}>
      {children}
    </ConfidenceClientProvider>
  );
}
