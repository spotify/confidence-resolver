'use client';
import { useContext, useEffect, useRef, useCallback } from 'react';
import { ConfidenceContext } from './ConfidenceProvider';

interface UseFlagOptionsAuto {
  expose?: true;
}

interface UseFlagOptionsManual {
  expose: false;
}

/**
 * React hook for accessing Confidence feature flag values.
 *
 * Automatically logs exposure when the component mounts.
 * Supports dot notation to access nested properties within a flag value.
 *
 * @param flagName - The flag name, optionally with dot notation for nested access (e.g., 'my-flag.config.enabled')
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
export function useFlag<T>(flagName: string, defaultValue: T): T {
  return useFlagDetails(flagName, defaultValue).value;
}

/**
 * React hook for accessing Confidence feature flag values with exposure control.
 *
 * By default, automatically logs exposure when the component mounts.
 * Use `{ expose: false }` for manual exposure control - useful when you want
 * to log exposure only after a user interaction.
 *
 * Supports dot notation to access nested properties within a flag value.
 *
 * @param flagName - The flag name, optionally with dot notation for nested access (e.g., 'my-flag.config.enabled')
 * @param defaultValue - Default value if flag or nested property is not found
 * @param options - Use `{ expose: false }` for manual exposure control
 * @returns Object with `value` and optional `expose` function (when using manual exposure)
 *
 * @example Auto exposure (default)
 * ```tsx
 * const { value: enabled } = useFlagDetails('my-feature', false);
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
 * @example Dot notation with manual exposure
 * ```tsx
 * const { value: maxItems, expose } = useFlagDetails('my-feature.config.maxItems', 10, { expose: false });
 * ```
 */
export function useFlagDetails<T>(
  flagName: string,
  defaultValue: T,
  options?: UseFlagOptionsAuto | UseFlagOptionsManual,
): FlagDetails<T> {
  const ctx = useContext(ConfidenceContext);
  const appliedRef = useRef(false);

  // Parse dot notation: first part is flag name, rest is path within value
  const [baseFlagName, ...path] = flagName.split('.');

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

  const resolvedValue = isAssignableTo(value, defaultValue) ? value : defaultValue;

  if (isManual) {
    return { value: resolvedValue, expose };
  } else {
    return { value: resolvedValue, expose: undefined };
  }
}

function hasKey<K extends string>(obj: object, key: K): obj is { [P in K]: unknown } {
  return key in obj;
}

function isAssignableTo<T>(value: unknown, schema: T): value is T {
  if (typeof schema !== typeof value) return false;
  if (typeof value === 'object' && typeof schema === 'object') {
    if (schema === null) return value === null;
    if (value === null) return false;
    if (Array.isArray(schema)) {
      if (!Array.isArray(value)) return false;
      if (schema.length === 0) return true;
      return value.every(item => isAssignableTo(item, schema[0]));
    }
    for (const [key, schemaValue] of Object.entries(schema)) {
      if (!hasKey(value, key)) return false;
      if (!isAssignableTo(value[key], schemaValue)) return false;
    }
  }
  return true;
}

export interface FlagDetails<T> {
  value: T;
  expose?: () => void;
}
