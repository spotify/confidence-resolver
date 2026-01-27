import type { JsonValue } from '@openfeature/core';
import type { ResolveFlagsResponse } from './proto/confidence/flags/resolver/v1/api';
import { ResolveReason } from './proto/confidence/flags/resolver/v1/types';
import { hasKey, base64FromBytes, bytesFromBase64 } from './util';
import { ErrorCode, FlagObject, FlagValue, ResolutionDetails, ResolutionReason } from './types';
import { Logger } from './logger';

const FLAG_PREFIX = 'flags/';

export default interface FlagBundle {
  flags: Record<string, ResolutionDetails<FlagObject | null> | undefined>;
  resolveId: string;
  resolveToken: string;
  errorCode?: ErrorCode;
  errorMessage?: string;
}

/** Encode Uint8Array to base64 string */
export const encodeToken: (bytes: Uint8Array) => string = base64FromBytes;

/** Decode base64 string to Uint8Array */
export const decodeToken: (base64: string) => Uint8Array = bytesFromBase64;

export function create({ resolveId, resolveToken, resolvedFlags }: ResolveFlagsResponse): FlagBundle {
  const flags = Object.fromEntries(
    resolvedFlags.map(({ flag, reason, variant, value, shouldApply }) => {
      const name = flag.slice(FLAG_PREFIX.length);
      const details: ResolutionDetails<FlagObject | null> = {
        reason: convertReason(reason),
        variant,
        value: value ?? null,
        shouldApply,
      };
      return [name, details];
    }),
  );

  return {
    flags,
    resolveId,
    resolveToken: encodeToken(resolveToken),
  };
}

export function error(errorCode: ErrorCode, errorMessage: string): FlagBundle {
  return {
    flags: {},
    resolveId: '',
    resolveToken: '',
    errorCode,
    errorMessage,
  };
}

export function resolve<T extends JsonValue>(
  bundle: FlagBundle,
  flagKey: string,
  defaultValue: T,
  logger?: Logger,
): ResolutionDetails<T> {
  const [flagName, ...path] = flagKey.split('.');
  const flag = bundle?.flags[flagName];

  if (bundle?.errorCode) {
    logger?.warn(`Flag evaluation for "%s" failed. %s %s`, flagKey, bundle.errorCode, bundle?.errorMessage);
    return {
      reason: 'ERROR',
      errorCode: bundle.errorCode,
      errorMessage: bundle.errorMessage,
      value: defaultValue,
      shouldApply: false,
    };
  }

  if (!flag) {
    logger?.warn(`Flag evaluation for '${flagKey}' failed: flag not found`);
    return {
      reason: 'ERROR',
      errorCode: ErrorCode.FLAG_NOT_FOUND,
      value: defaultValue,
      shouldApply: false,
    };
  }

  let value: FlagValue = flag.value;
  for (let i = 0; i < path.length; i++) {
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
      return {
        reason: 'ERROR',
        value: defaultValue,
        errorCode: ErrorCode.TYPE_MISMATCH,
        errorMessage: `resolved value is not an object at ${[flagName, ...path.slice(0, i)].join('.')}`,
        shouldApply: false,
      };
    }
    value = value[path[i]];
  }

  try {
    const validated = evaluateAssignment(value, defaultValue, [flagName, ...path]);
    return {
      ...flag,
      value: validated,
    };
  } catch (e) {
    return {
      reason: 'ERROR',
      value: defaultValue,
      errorCode: ErrorCode.TYPE_MISMATCH,
      errorMessage: String(e),
      shouldApply: false,
    };
  }
}

export function evaluateAssignment(resolvedValue: FlagValue, defaultValue: null, path: string[]): FlagValue;
export function evaluateAssignment<T extends JsonValue>(resolvedValue: FlagValue, defaultValue: T, path: string[]): T;
export function evaluateAssignment<T extends JsonValue>(resolvedValue: FlagValue, defaultValue: T, path: string[]): T {
  const resolvedType = typeof resolvedValue;
  const defaultType = typeof defaultValue;

  // Arrays are not supported
  if (Array.isArray(defaultValue)) {
    throw `arrays are not supported as flag values at ${path.join('.')}`;
  }

  // If default is null, any value is acceptable
  if (defaultValue === null) return resolvedValue as T;

  // If resolved is null, substitute default
  if (resolvedValue === null) return defaultValue;

  // Type mismatch check
  if (resolvedType !== defaultType) {
    throw `resolved value (${resolvedType}) isn't assignable to default type (${defaultType}) at ${path.join('.')}`;
  }

  if (typeof resolvedValue === 'object') {
    const result: Record<string, FlagValue> = { ...resolvedValue };
    for (const [key, value] of Object.entries(defaultValue as Record<string, JsonValue>)) {
      if (!hasKey(resolvedValue, key)) {
        throw `resolved value is missing field "${key}" at ${path.join('.')}`;
      }
      result[key] = evaluateAssignment(resolvedValue[key], value, [...path, key]) as FlagValue;
    }
    return result as T;
  }

  // Primitives - already validated type match
  return resolvedValue as T;
}

function convertReason(reason: ResolveReason): ResolutionReason {
  switch (reason) {
    case ResolveReason.RESOLVE_REASON_ERROR:
      return 'ERROR';
    case ResolveReason.RESOLVE_REASON_FLAG_ARCHIVED:
      return 'FLAG_ARCHIVED';
    case ResolveReason.RESOLVE_REASON_MATCH:
      return 'MATCH';
    case ResolveReason.RESOLVE_REASON_NO_SEGMENT_MATCH:
      return 'NO_SEGMENT_MATCH';
    case ResolveReason.RESOLVE_REASON_TARGETING_KEY_ERROR:
      return 'TARGETING_KEY_ERROR';
    case ResolveReason.RESOLVE_REASON_NO_TREATMENT_MATCH:
      return 'NO_TREATMENT_MATCH';
    default:
      return 'UNSPECIFIED';
  }
}
