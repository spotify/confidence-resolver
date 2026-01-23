import type { FlagValue, ResolutionDetails } from '@openfeature/core';

/**
 * A bundle of resolved flags that can be serialized and passed to client components.
 */
export interface FlagBundle {
  /** Map of flag names to their resolved details */
  flags: Record<string, ResolutionDetails<FlagValue>>;
  /** Base64-encoded resolve token for apply calls */
  resolveToken: string;
  /** Unique identifier for this resolution */
  resolveId: string;
}
