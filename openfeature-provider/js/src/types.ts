/**
 * Details of a resolved flag value.
 */
export interface ResolvedFlagDetails {
  value: unknown;
  variant?: string;
  reason: string;
  errorCode?: string;
  errorMessage?: string;
}

/**
 * A bundle of resolved flags that can be serialized and passed to client components.
 */
export interface FlagBundle {
  /** Map of flag names to their resolved details */
  flags: Record<string, ResolvedFlagDetails>;
  /** Base64-encoded resolve token for apply calls */
  resolveToken: string;
  /** Unique identifier for this resolution */
  resolveId: string;
}
