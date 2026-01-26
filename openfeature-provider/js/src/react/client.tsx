'use client';

import { createContext, useContext, useEffect, useCallback } from 'react';
import type { EvaluationDetails, FlagValue } from '@openfeature/core';
import type FlagBundleType from '../flag-bundle';
import * as FlagBundle from '../flag-bundle';
import { devWarn } from '../util';

type FlagBundle = FlagBundleType;

type ApplyFn = (flagName: string) => Promise<void>;

interface ConfidenceContextValue {
  bundle: FlagBundle;
  apply: ApplyFn;
}

const ConfidenceContext = createContext<ConfidenceContextValue | null>(null);

const warnedFlags = new Set<string>();
const appliedFlags = new Set<string>();

/** @internal - Reset state for testing */
export function resetAppliedFlags(): void {
  appliedFlags.clear();
  warnedFlags.clear();
}

/** @internal */
export interface ConfidenceClientProviderProps {
  bundle: FlagBundle;
  apply: ApplyFn;
  children: React.ReactNode;
}

/** @internal */
export function ConfidenceClientProvider({
  bundle,
  apply,
  children,
}: ConfidenceClientProviderProps): React.ReactElement {
  return <ConfidenceContext.Provider value={{ bundle, apply }}>{children}</ConfidenceContext.Provider>;
}

interface UseFlagOptions {
  /** Set to false for manual exposure control. Default is true (auto-expose on mount). */
  expose?: boolean;
}

/**
 * Details returned by useFlagDetails hook.
 * Always includes an expose function for manual exposure logging.
 */
export interface ClientEvaluationDetails<T extends FlagValue> extends EvaluationDetails<T> {
  /** Function to manually log exposure. No-op if already auto-exposed or if called multiple times. */
  expose: () => void;
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
 * @returns EvaluationDetails with value, flagKey, flagMetadata, variant, reason, errorCode, errorMessage, and expose function.
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
  options?: UseFlagOptions,
): ClientEvaluationDetails<T> {
  const ctx = useContext(ConfidenceContext);

  // Parse dot notation: first part is flag name, rest is path within value
  const [baseFlagName, ...path] = flagKey.split('.');

  // Warn if no provider is present (only once per flag)
  if (!ctx && !warnedFlags.has(flagKey)) {
    warnedFlags.add(flagKey);
    devWarn(`[Confidence] useFlagDetails("${flagKey}") called without a ConfidenceProvider. Returning default value.`);
  }

  const resolution = FlagBundle.resolve(ctx?.bundle, flagKey, defaultValue);
  const autoExpose = options?.expose !== false;

  // Internal function to actually log exposure
  const doExpose = useCallback(() => {
    if (ctx && !appliedFlags.has(baseFlagName)) {
      appliedFlags.add(baseFlagName);
      ctx.apply(baseFlagName);
    }
  }, [ctx, baseFlagName]);

  // Auto exposure effect
  useEffect(() => {
    if (autoExpose && resolution.shouldApply) {
      doExpose();
    }
  }, [autoExpose, doExpose, resolution.shouldApply]);

  // Expose function returned to caller
  const expose = useCallback(() => {
    if (!resolution.shouldApply) {
      devWarn(
        `[Confidence] attempt to expose an unmatched flag ${baseFlagName}: ${resolution.reason} ${resolution.errorCode}`,
      );
    } else if (autoExpose) {
      devWarn(`[Confidence] expose() called on "${flagKey}" but auto-exposure is enabled. Call is ignored.`);
    } else {
      doExpose();
    }
  }, [autoExpose, baseFlagName, doExpose, flagKey, resolution.shouldApply, resolution.reason, resolution.errorCode]);

  return {
    flagKey,
    flagMetadata: {},
    ...resolution,
    expose,
  };
}
