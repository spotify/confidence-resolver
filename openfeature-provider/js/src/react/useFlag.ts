'use client';
import { useContext, useEffect, useRef, useCallback } from 'react';
import { ConfidenceContext } from './ConfidenceProvider';

interface UseFlagOptionsAuto {
  skipExposure?: false;
}

interface UseFlagOptionsManual {
  skipExposure: true;
}

interface ManualExposureResult<T> {
  value: T;
  expose: () => void;
}

// Overload: auto exposure (default) - returns value directly
export function useFlag<T>(flagName: string, defaultValue: T): T;
export function useFlag<T>(flagName: string, defaultValue: T, options: UseFlagOptionsAuto): T;

// Overload: manual exposure - returns { value, expose }
export function useFlag<T>(
  flagName: string,
  defaultValue: T,
  options: UseFlagOptionsManual,
): ManualExposureResult<T>;

/**
 * React hook for accessing Confidence feature flag values.
 *
 * By default, automatically logs exposure when the component mounts.
 * Use `skipExposure: true` to defer exposure until you call `expose()`.
 *
 * @param flagName - The name of the flag to access
 * @param defaultValue - Default value if flag is not found
 * @param options - Optional settings. Use `{ skipExposure: true }` for manual exposure control.
 * @returns The flag value (auto exposure) or `{ value, expose }` (manual exposure)
 *
 * @example Auto exposure (default)
 * ```tsx
 * const enabled = useFlag('my-feature.enabled', false);
 * ```
 *
 * @example Manual exposure
 * ```tsx
 * const { value: enabled, expose } = useFlag('my-feature.enabled', false, { skipExposure: true });
 *
 * const handleClick = () => {
 *   if (enabled) {
 *     expose(); // Log exposure only when user interacts
 *     doSomething();
 *   }
 * };
 * ```
 */
export function useFlag<T>(
  flagName: string,
  defaultValue: T,
  options?: { skipExposure?: boolean },
): T | ManualExposureResult<T> {
  const ctx = useContext(ConfidenceContext);
  const appliedRef = useRef(false);
  const skipExposure = options?.skipExposure ?? false;

  // Auto exposure effect
  useEffect(() => {
    if (ctx && !appliedRef.current && !skipExposure) {
      appliedRef.current = true;
      ctx.apply(flagName);
    }
  }, [ctx, flagName, skipExposure]);

  // Manual expose function (bound to flag name)
  const expose = useCallback(() => {
    if (ctx && !appliedRef.current) {
      appliedRef.current = true;
      ctx.apply(flagName);
    }
  }, [ctx, flagName]);

  const flag = ctx?.bundle.flags[flagName];
  const value = (flag?.value as T) ?? defaultValue;

  if (skipExposure) {
    return { value, expose };
  }

  return value;
}
