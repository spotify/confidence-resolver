import {
  OpenFeature,
  type Provider,
  type EvaluationContext,
  type EvaluationDetails,
  type JsonValue,
} from '@openfeature/server-sdk';
import { isConfidenceProvider, type ConfidenceBundleProvider } from '../provider-contract';
import { ConfidenceClientProvider } from './client';
import { devWarn } from '../util';
import type FlagBundleType from '../flag-bundle';
import * as FlagBundle from '../flag-bundle';
import { ErrorCode } from '../types';

type FlagBundle = FlagBundleType;

function getConfidenceProvider(providerName?: string): Provider {
  return providerName ? OpenFeature.getProvider(providerName) : OpenFeature.getProvider();
}

/**
 * ConfidenceProvider requires a provider implementing the Confidence bundle
 * API (`resolve`, `applyFlag`, `evaluate`) — the state-based
 * ConfidenceServerProviderLocal or the module-based ConfidenceModuleProvider.
 * Make sure you have registered the provider with OpenFeature before rendering.
 */
function isConfidenceBundleProvider(provider: Provider): provider is ConfidenceBundleProvider {
  if (!isConfidenceProvider(provider)) {
    devWarn(
      `ConfidenceProvider requires a Confidence provider (ConfidenceServerProviderLocal or ConfidenceModuleProvider), but got ${
        provider?.metadata?.name ?? 'undefined'
      }. ` + 'Make sure you have registered the provider with OpenFeature before rendering.',
    );
    return false;
  }
  return true;
}

export interface ConfidenceProviderProps {
  /** The evaluation context for flag resolution */
  context: EvaluationContext;
  /** Optional provider name. If not specified, uses the default provider. */
  providerName?: string;
  /** Flag names to resolve. If not specified, resolves all flags. */
  flags?: string[];
  /** Child components */
  children: React.ReactNode;
}

/**
 * React Server Component that resolves flags and provides them to client components.
 *
 * This component resolves all specified flags in a single call on the server and
 * passes the results to client components via React Context. Client components
 * can then access flag values using the `useFlag` and `useFlagDetails` hooks
 * from `react-client`.
 *
 * Flags are resolved **without** logging exposure. Exposure is logged when client
 * components call the hooks (automatically on mount, or manually via `expose()`).
 *
 * Must be used with a Confidence provider (`ConfidenceServerProviderLocal` or
 * `ConfidenceModuleProvider`) registered with OpenFeature.
 *
 * @example
 * ```tsx
 * // app/layout.tsx
 * import { ConfidenceProvider } from '@spotify-confidence/openfeature-server-provider-local/react-server';
 *
 * export default async function RootLayout({ children }: { children: React.ReactNode }) {
 *   const context = {
 *     targetingKey: 'user-123',
 *     country: 'US',
 *   };
 *
 *   return (
 *     <html>
 *       <body>
 *         <ConfidenceProvider context={context} flags={['checkout-flow', 'promo-banner']}>
 *           {children}
 *         </ConfidenceProvider>
 *       </body>
 *     </html>
 *   );
 * }
 * ```
 */
export async function ConfidenceProvider({
  context,
  providerName,
  flags = [],
  children,
}: ConfidenceProviderProps): Promise<React.ReactElement> {
  const provider = getConfidenceProvider(providerName);

  let bundle: FlagBundle;
  if (isConfidenceBundleProvider(provider)) {
    bundle = await provider.resolve(context, flags);
  } else {
    bundle = FlagBundle.error(
      ErrorCode.GENERAL,
      `The registered OpenFeatureProvider (${providerName ?? 'default'}) is not a Confidence provider: ${
        provider?.metadata?.name ?? 'undefined'
      }`,
    );
  }

  async function applyFlag(flagName: string): Promise<void> {
    'use server';

    const serverProvider = getConfidenceProvider(providerName);

    if (!bundle.errorCode && isConfidenceProvider(serverProvider)) {
      serverProvider.applyFlag(bundle.resolveToken, flagName);
    }
  }

  // The resolve token is sensitive (it carries the full evaluation context and
  // resolved variants) and the client never reads it — strip it before serializing
  // the bundle into the RSC payload. The server action above keeps the original
  // bundle in its closure, which Next.js 14+ encrypts.
  const clientBundle: FlagBundle = { ...bundle, resolveToken: '' };

  return (
    <ConfidenceClientProvider bundle={clientBundle} apply={applyFlag}>
      {children}
    </ConfidenceClientProvider>
  );
}

/**
 * Evaluate a flag in a React Server Component and get full evaluation details.
 *
 * This function evaluates the flag and **immediately logs exposure** since server
 * components render once without hydration. Use this when you need access to
 * variant, reason, or error information.
 *
 * Supports dot notation to access nested properties within a flag value
 * (e.g., 'my-flag.config.enabled').
 *
 * @param flagKey - The flag key, optionally with dot notation for nested access
 * @param defaultValue - Default value returned if flag is not found or type doesn't match
 * @param context - Evaluation context containing targetingKey and other attributes
 * @param providerName - Optional named provider (uses default provider if not specified)
 * @returns Promise resolving to EvaluationDetails with value, variant, reason, and error info
 *
 * @example
 * ```tsx
 * // app/page.tsx (Server Component)
 * import { getFlagDetails } from '@spotify-confidence/openfeature-server-provider-local/react-server';
 *
 * export default async function Page() {
 *   const { value, variant, reason } = await getFlagDetails(
 *     'checkout-flow.enabled',
 *     false,
 *     { targetingKey: 'user-123' }
 *   );
 *
 *   console.log(`Resolved to variant ${variant} because: ${reason}`);
 *   return value ? <NewCheckout /> : <OldCheckout />;
 * }
 * ```
 */
export async function getFlagDetails<T extends JsonValue>(
  flagKey: string,
  defaultValue: T,
  context: EvaluationContext,
  providerName?: string,
): Promise<EvaluationDetails<T>> {
  const provider = getConfidenceProvider(providerName);
  if (!isConfidenceProvider(provider)) {
    return {
      flagKey,
      flagMetadata: {},
      value: defaultValue,
      reason: 'ERROR',
      errorCode: ErrorCode.GENERAL,
      errorMessage: 'Provider is not a Confidence provider',
    };
  }
  const details = await provider.evaluate(flagKey, defaultValue, context);
  return {
    flagKey,
    flagMetadata: {},
    ...details,
  };
}

/**
 * Evaluate a flag in a React Server Component and get the value.
 *
 * This function evaluates the flag and **immediately logs exposure** since server
 * components render once without hydration. This is the simplest way to get a
 * flag value in server components.
 *
 * Supports dot notation to access nested properties within a flag value
 * (e.g., 'my-flag.config.enabled').
 *
 * @param flagKey - The flag key, optionally with dot notation for nested access
 * @param defaultValue - Default value returned if flag is not found or type doesn't match
 * @param context - Evaluation context containing targetingKey and other attributes
 * @param providerName - Optional named provider (uses default provider if not specified)
 * @returns Promise resolving to the flag value
 *
 * @example
 * ```tsx
 * // app/page.tsx (Server Component)
 * import { getFlag } from '@spotify-confidence/openfeature-server-provider-local/react-server';
 *
 * export default async function Page() {
 *   const showNewLayout = await getFlag(
 *     'page-layout.useNewDesign',
 *     false,
 *     { targetingKey: 'user-123' }
 *   );
 *
 *   return showNewLayout ? <NewLayout /> : <OldLayout />;
 * }
 * ```
 *
 * @see getFlagDetails for accessing variant, reason, and error information
 */
export async function getFlag<T extends JsonValue>(
  flagKey: string,
  defaultValue: T,
  context: EvaluationContext,
  providerName?: string,
): Promise<T> {
  return (await getFlagDetails(flagKey, defaultValue, context, providerName)).value;
}
