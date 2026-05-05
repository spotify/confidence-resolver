import type FlagBundleType from '../flag-bundle';
import type { ConfidenceServerProviderLocal } from '../ConfidenceServerProviderLocal';

export type FlagBundle = FlagBundleType;
export type ConfidenceProvider = ConfidenceServerProviderLocal;

/**
 * Server-to-client transport payload returned from `withConfidence` and
 * consumed by `<ConfidencePagesProvider>`. Structurally a `FlagBundle` whose
 * `resolveToken` has been sealed (AES-GCM) — the apply API route is the only
 * thing that opens it.
 */
export type ConfidencePageProps = FlagBundle;
