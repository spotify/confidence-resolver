/**
 * Types for the Confidence React package.
 * These types are duplicated from the server-provider to avoid a runtime dependency.
 * @module
 */

/**
 * A resolved flag value with metadata
 * @public
 */
export interface ResolvedFlagDetails {
  value: unknown;
  variant?: string;
  reason: string;
  errorCode?: string;
  errorMessage?: string;
}

/**
 * A bundle of pre-resolved flag values for client-side consumption
 * @public
 */
export interface FlagBundle {
  flags: Record<string, ResolvedFlagDetails>;
  resolveToken: string; // base64 encoded
  resolveId: string;
}

/**
 * Function type for applying/exposing a flag
 * @public
 */
export type ApplyFn = (flagName: string) => Promise<void>;

/**
 * Result of resolveFlagBundle with a pre-bound apply function
 * @public
 */
export interface FlagBundleResult {
  bundle: FlagBundle;
  applyFlag: ApplyFn;
}
