import type FlagBundleType from '../flag-bundle';
import type { ConfidenceBundleProvider } from '../provider-contract';

export type FlagBundle = FlagBundleType;
/** Any provider implementing the Confidence bundle API (state-based or module-based). */
export type ConfidenceProvider = ConfidenceBundleProvider;

/**
 * Server-to-client transport payload returned from `withConfidence` and
 * consumed by `<ConfidencePagesProvider>`. Structurally a `FlagBundle` whose
 * `resolveToken` has been sealed (AES-GCM) — the apply API route is the only
 * thing that opens it.
 */
export type ConfidencePageProps = FlagBundle;
