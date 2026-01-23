'use client';

import { createContext, useContext, useEffect, useRef, useCallback } from 'react';
import type { EvaluationDetails, FlagValue } from '@openfeature/core';
import type { FlagBundle } from '../types';
import { isAssignableTo } from '../type-utils';

type ApplyFn = (flagName: string) => Promise<void>;

interface ConfidenceContextValue {
  bundle: FlagBundle;
  apply: ApplyFn;
}

const ConfidenceContext = createContext<ConfidenceContextValue | null>(null);

const warnedFlags = new Set<string>();

export interface ConfidenceClientProviderProps {
  bundle: FlagBundle;
  apply: ApplyFn;
  children: React.ReactNode;
}

export function ConfidenceClientProvider({
  bundle,
  apply,
  children,
}: ConfidenceClientProviderProps): React.ReactElement {
  return <ConfidenceContext.Provider value={{ bundle, apply }}>{children}</ConfidenceContext.Provider>;
}

interface UseFlagOptionsAuto {
  expose?: true;
}

interface UseFlagOptionsManual {
  expose: false;
}

/**
 * Details returned by useFlagDetails hook.
 * Extends OpenFeature's EvaluationDetails with an optional expose function for manual exposure control.
 */
export interface ClientEvaluationDetails<T extends FlagValue> extends EvaluationDetails<T> {
  /** Function to manually log exposure. Only present when using { expose: false } option. */
  expose?: () => void;
}

/**
 * React hook to get the list of all flag names available in the bundle.
 *
 * @returns Array of flag names, or empty array if no provider is present
 *
 * @example
 * ```tsx
 * const flagNames = useFlagNames();
 * // ['feature-a', 'feature-b', 'experiment-1']
 * ```
 */
export function useFlagNames(): string[] {
  const ctx = useContext(ConfidenceContext);
  if (!ctx) return [];
  return Object.keys(ctx.bundle.flags);
}

/**
 * React hook for accessing Confidence feature flag values.
 *
 * Automatically logs exposure when the component mounts.
 * Supports dot notation to access nested properties within a flag value.
 *
 * @param flagKey - The flag key, optionally with dot notation for nested access (e.g., 'my-flag.config.enabled')
 * @param defaultValue - Default value if flag or nested property is not found
 * @returns The flag value (or nested property value)
 *
 * @example Basic usage
 * ```tsx
 * const enabled = useFlag('my-feature', false);
 * ```
 *
 * @example Dot notation for nested properties
 * ```tsx
 * // Flag value: { config: { maxItems: 10, enabled: true } }
 * const maxItems = useFlag('my-feature.config.maxItems', 5);
 * const enabled = useFlag('my-feature.config.enabled', false);
 * ```
 *
 * @see useFlagDetails for manual exposure control
 */
export function useFlag<T extends FlagValue>(flagKey: string, defaultValue: T): T {
  return useFlagDetails(flagKey, defaultValue).value;
}

/**
 * React hook for accessing Confidence feature flag values with full details.
 *
 * Returns the flag value along with variant, reason, and error information.
 * By default, automatically logs exposure when the component mounts.
 * Use `{ expose: false }` for manual exposure control.
 *
 * Supports dot notation to access nested properties within a flag value.
 *
 * @param flagKey - The flag key, optionally with dot notation for nested access (e.g., 'my-flag.config.enabled')
 * @param defaultValue - Default value if flag or nested property is not found
 * @param options - Use `{ expose: false }` for manual exposure control
 * @returns ClientEvaluationDetails with value, flagKey, flagMetadata, variant, reason, errorCode, errorMessage, and optional expose function
 *
 * @example Auto exposure with full details
 * ```tsx
 * const { value, variant, reason } = useFlagDetails('my-feature', false);
 * console.log(`Got ${value} from variant ${variant}, reason: ${reason}`);
 * ```
 *
 * @example Manual exposure
 * ```tsx
 * const { value: enabled, expose } = useFlagDetails('my-feature', false, { expose: false });
 *
 * const handleClick = () => {
 *   if (enabled) {
 *     expose(); // Log exposure only when user interacts
 *     doSomething();
 *   }
 * };
 * ```
 *
 * @example Error handling
 * ```tsx
 * const { value, errorCode } = useFlagDetails('my-feature', false);
 * if (errorCode === 'FLAG_NOT_FOUND') {
 *   console.warn('Flag not configured');
 * }
 * ```
 */
export function useFlagDetails<T extends FlagValue>(
  flagKey: string,
  defaultValue: T,
  options?: UseFlagOptionsAuto | UseFlagOptionsManual,
): ClientEvaluationDetails<T> {
  const ctx = useContext(ConfidenceContext);
  const appliedRef = useRef(false);

  // Parse dot notation: first part is flag name, rest is path within value
  const [baseFlagName, ...path] = flagKey.split('.');

  // Warn if no provider is present (only once per flag)
  if (!ctx && !warnedFlags.has(flagKey)) {
    warnedFlags.add(flagKey);
    console.warn(
      `[Confidence] useFlagDetails("${flagKey}") called without a ConfidenceProvider. ` + `Returning default value.`,
    );
  }

  const isManual = options?.expose === false;

  // Auto exposure effect - apply with just the flag name
  useEffect(() => {
    if (ctx && !isManual && !appliedRef.current) {
      appliedRef.current = true;
      ctx.apply(baseFlagName);
    }
  }, [ctx, baseFlagName, isManual]);

  // Manual expose function (bound to flag name)
  const expose = useCallback(() => {
    if (ctx && !appliedRef.current) {
      appliedRef.current = true;
      ctx.apply(baseFlagName);
    }
  }, [ctx, baseFlagName]);

  const flag = ctx?.bundle.flags[baseFlagName];
  let value: unknown = flag?.value;

  // Navigate the path within the value
  for (const key of path) {
    if (value === null || value === undefined || typeof value !== 'object') {
      value = undefined;
      break;
    }
    value = (value as Record<string, unknown>)[key];
  }

  const resolvedValue = isAssignableTo(value, defaultValue, true) ? value : defaultValue;

  // Get details from the flag, or use defaults for missing flags
  const variant = flag?.variant;
  const reason = flag?.reason ?? 'ERROR';
  const errorCode = flag?.errorCode ?? (flag ? undefined : 'FLAG_NOT_FOUND');
  const errorMessage = flag?.errorMessage;

  const baseDetails: ClientEvaluationDetails<T> = {
    flagKey,
    flagMetadata: {},
    value: resolvedValue,
    variant,
    reason,
    errorCode,
    errorMessage,
  };

  if (isManual) {
    return { ...baseDetails, expose };
  } else {
    return baseDetails;
  }
}
