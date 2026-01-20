/**
 * A resolved flag value with metadata
 * @public
 */
export interface ResolvedFlagValue {
  value: unknown;
  variant?: string;
  reason: string;
}

/**
 * A bundle of pre-resolved flag values for client-side consumption
 * @public
 */
export interface FlagBundle {
  flags: Record<string, ResolvedFlagValue>;
  resolveToken: string; // base64 encoded
  resolveId: string;
}

/**
 * Function type for applying/exposing a flag
 * @public
 */
export type ApplyFn = (flagName: string) => Promise<void>;

/**
 * Result of createFlagBundle with a pre-bound apply function
 * @public
 */
export interface FlagBundleResult {
  bundle: FlagBundle;
  applyFlag: ApplyFn;
}
